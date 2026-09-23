// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::cmp;
use core::convert::Infallible as Never;
use core::fmt::Debug;
use core::num::NonZeroU16;

use net_types::ip::{
    GenericOverIp, Ip, IpAddress, IpInvariant, IpVersionMarker, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr,
};
use netstack3_base::{
    DynamicNetworkPartialSerializer, DynamicNetworkSerializer, MalformedFlags,
    NetworkPartialSerializer, NetworkSerializationContext, NetworkSerializer, Options, PayloadLen,
    SegmentHeader,
};
use packet::{
    Buf, Buffer, BufferMut, BufferProvider, BufferViewMut, ContiguousBuffer, DynPartialSerializer,
    DynSerializer, EitherSerializer, EmptyBuf, GrowBufferMut, InnerSerializer, LayoutBufferAlloc,
    NestablePacketBuilder as _, NestableSerializer, Nested, PacketConstraints, ParsablePacket,
    ParseBuffer, ParseMetadata, PartialSerializeResult, PartialSerializer, SerializationContext,
    SerializeError, Serializer, SliceBufViewMut, TruncatingSerializer,
};
use packet_formats::TRANSPORT_HEADER_MAX_SIZE;
use packet_formats::icmp::mld::{
    MulticastListenerDone, MulticastListenerQuery, MulticastListenerQueryV2,
    MulticastListenerReport, MulticastListenerReportV2,
};
use packet_formats::icmp::ndp::options::NdpOptionBuilder;
use packet_formats::icmp::ndp::{
    NeighborAdvertisement, NeighborSolicitation, Redirect, RouterAdvertisement, RouterSolicitation,
};
use packet_formats::icmp::{
    self, IcmpDestUnreachable, IcmpEchoReply, IcmpEchoRequest, IcmpPacketBuilder, IcmpPacketRaw,
    IcmpPacketTypeRaw as _, IcmpTimeExceeded, Icmpv4MessageType, Icmpv4PacketRaw,
    Icmpv4ParameterProblem, Icmpv4Redirect, Icmpv4TimestampReply, Icmpv4TimestampRequest,
    Icmpv6MessageType, Icmpv6PacketRaw, Icmpv6PacketTooBig, Icmpv6ParameterProblem,
};
use packet_formats::igmp::messages::IgmpMembershipReportV3Builder;
use packet_formats::igmp::{self, IgmpPacketBuilder};
use packet_formats::ip::{IpExt, IpPacketBuilder, IpProto, Ipv4Proto, Ipv6Proto};
use packet_formats::ipv4::{Ipv4Header, Ipv4Packet, Ipv4PacketRaw};
use packet_formats::ipv6::{Ipv6Header, Ipv6Packet, Ipv6PacketRaw};
use packet_formats::tcp::options::TcpOptionsBuilder;
use packet_formats::tcp::{TcpSegmentBuilderWithOptions, TcpSegmentRaw};
use packet_formats::udp::{UdpPacketBuilder, UdpPacketRaw};
use zerocopy::{SplitByteSlice, SplitByteSliceMut};

use crate::conntrack;

/// An IP extension trait for the filtering crate.
pub trait FilterIpExt: IpExt {
    /// A marker type to add an [`IpPacket`] bound to [`Self::Packet`].
    type FilterIpPacket<B: SplitByteSliceMut>: FilterIpPacket<Self>;

    /// A marker type to add an [`IpPacket`] bound to
    /// [`Self::PacketRaw`].
    type FilterIpPacketRaw<B: SplitByteSliceMut>: IpPacket<Self>;

    /// A no-op conversion to help the compiler identify that [`Self::Packet`]
    /// actually implements [`IpPacket`].
    fn as_filter_packet<B: SplitByteSliceMut>(
        packet: &mut Self::Packet<B>,
    ) -> &mut Self::FilterIpPacket<B>;

    /// The same as [`FilterIpExt::as_filter_packet`], but for owned values.
    fn as_filter_packet_owned<B: SplitByteSliceMut>(
        packet: Self::Packet<B>,
    ) -> Self::FilterIpPacket<B>;

    /// The same as [`FilterIpExt::as_filter_packet_owned`], but for owned raw
    /// values.
    fn as_filter_packet_raw_owned<B: SplitByteSliceMut>(
        packet: Self::PacketRaw<B>,
    ) -> Self::FilterIpPacketRaw<B>;
}

impl FilterIpExt for Ipv4 {
    type FilterIpPacket<B: SplitByteSliceMut> = Ipv4Packet<B>;
    type FilterIpPacketRaw<B: SplitByteSliceMut> = Ipv4PacketRaw<B>;

    #[inline]
    fn as_filter_packet<B: SplitByteSliceMut>(packet: &mut Ipv4Packet<B>) -> &mut Ipv4Packet<B> {
        packet
    }

    #[inline]
    fn as_filter_packet_owned<B: SplitByteSliceMut>(
        packet: Self::Packet<B>,
    ) -> Self::FilterIpPacket<B> {
        packet
    }

    #[inline]
    fn as_filter_packet_raw_owned<B: SplitByteSliceMut>(
        packet: Self::PacketRaw<B>,
    ) -> Self::FilterIpPacketRaw<B> {
        packet
    }
}

impl FilterIpExt for Ipv6 {
    type FilterIpPacket<B: SplitByteSliceMut> = Ipv6Packet<B>;
    type FilterIpPacketRaw<B: SplitByteSliceMut> = Ipv6PacketRaw<B>;

    #[inline]
    fn as_filter_packet<B: SplitByteSliceMut>(packet: &mut Ipv6Packet<B>) -> &mut Ipv6Packet<B> {
        packet
    }

    #[inline]
    fn as_filter_packet_owned<B: SplitByteSliceMut>(
        packet: Self::Packet<B>,
    ) -> Self::FilterIpPacket<B> {
        packet
    }

    #[inline]
    fn as_filter_packet_raw_owned<B: SplitByteSliceMut>(
        packet: Self::PacketRaw<B>,
    ) -> Self::FilterIpPacketRaw<B> {
        packet
    }
}

/// An IP packet that provides header inspection.
pub trait IpPacket<I: FilterIpExt> {
    /// The type that provides access to transport-layer header inspection, if a
    /// transport header is contained in the body of the IP packet.
    type TransportPacket<'a>: MaybeTransportPacket
    where
        Self: 'a;

    /// The type that provides access to transport-layer header modification, if a
    /// transport header is contained in the body of the IP packet.
    type TransportPacketMut<'a>: MaybeTransportPacketMut<I>
    where
        Self: 'a;

    /// The type that provides access to IP- and transport-layer information
    /// within an ICMP error packet, if this IP packet contains one.
    type IcmpError<'a>: MaybeIcmpErrorPayload<I>
    where
        Self: 'a;

    /// The type that provides mutable access to the message within an ICMP
    /// error packet, if this IP packet contains one.
    type IcmpErrorMut<'a>: MaybeIcmpErrorMut<I>
    where
        Self: 'a;

    /// The source IP address of the packet.
    fn src_addr(&self) -> I::Addr;

    /// Sets the source IP address of the packet.
    fn set_src_addr(&mut self, addr: I::Addr);

    /// The destination IP address of the packet.
    fn dst_addr(&self) -> I::Addr;

    /// Sets the destination IP address of the packet.
    fn set_dst_addr(&mut self, addr: I::Addr);

    /// The IP protocol of the packet.
    fn protocol(&self) -> Option<I::Proto>;

    /// Returns a type that provides access to the transport-layer packet contained
    /// in the body of the IP packet, if one exists.
    ///
    /// This method returns an owned type parameterized on a lifetime that is tied
    /// to the lifetime of Self, rather than, for example, a reference to a
    /// non-parameterized type (`&Self::TransportPacket`). This is because
    /// implementors may need to parse the transport header from the body of the IP
    /// packet and materialize the results into a new type when this is called, but
    /// that type may also need to retain a reference to the backing buffer in order
    /// to modify the transport header.
    fn maybe_transport_packet<'a>(&'a self) -> Self::TransportPacket<'a>;

    /// Returns a type that provides the ability to modify the transport-layer
    /// packet contained in the body of the IP packet, if one exists.
    ///
    /// This method returns an owned type parameterized on a lifetime that is tied
    /// to the lifetime of Self, rather than, for example, a reference to a
    /// non-parameterized type (`&Self::TransportPacketMut`). This is because
    /// implementors may need to parse the transport header from the body of the IP
    /// packet and materialize the results into a new type when this is called, but
    /// that type may also need to retain a reference to the backing buffer in order
    /// to modify the transport header.
    fn transport_packet_mut<'a>(&'a mut self) -> Self::TransportPacketMut<'a>;

    /// Returns a type that provides the ability to access the IP- and
    /// transport-layer headers contained within the body of the ICMP error
    /// message, if one exists in this packet.
    ///
    /// NOTE: See the note on [`IpPacket::maybe_transport_packet`].
    fn maybe_icmp_error<'a>(&'a self) -> Self::IcmpError<'a>;

    /// Returns a type that provides the ability to modify the IP- and
    /// transport-layer headers contained within the body of the ICMP error
    /// message, if one exists in this packet.
    ///
    /// NOTE: See the note on [`IpPacket::transport_packet_mut`].
    fn icmp_error_mut<'a>(&'a mut self) -> Self::IcmpErrorMut<'a>;

    /// The header information to be used for connection tracking.
    ///
    /// For the transport header, this currently returns the same information as
    /// [`IpPacket::maybe_transport_packet`], but may be different for packets
    /// such as ICMP errors. In that case, we care about the inner IP packet for
    /// connection tracking, but use the outer header for filtering.
    ///
    /// Subtlety: For ICMP packets, only request/response messages will have
    /// a transport packet defined (and currently only ECHO messages do). This
    /// gets us basic tracking for free, and lets us implicitly ignore ICMP
    /// errors, which are not meant to be tracked.
    ///
    /// If other ICMP message types eventually have TransportPacket impls, then
    /// this would lead to multiple message types being mapped to the same tuple
    /// if they happen to have the same ID.
    fn conntrack_packet(&self) -> Option<conntrack::PacketMetadata<I>> {
        if let Some(payload) = self.maybe_icmp_error().icmp_error_payload() {
            // Checks whether it's reasonable that `payload` is a payload inside
            // an ICMP error with tuple `outer`.
            //
            // An ICMP error can be returned from any router between the sender
            // and the receiver, from the receiver itself, or from the netstack
            // on send (for synthetic errors). Therefore, an originating packet
            // (A -> B) could end up in ICMP errors that look like:
            //
            // B -> A | A -> B
            // R -> A | A -> B
            // A -> A | A -> B
            //
            // where R is some router along the path from A to B.
            //
            // Notice that in both of these cases the destination address is
            // always A. There's no more check we can make, even if we had
            // access to conntrack data for the payload tuple. It's valid for us
            // to compare addresses from the outer packet and the inner payload,
            // even in the presence of NAT, because we can think of the payload
            // and outer tuples as having come from the same "side" of the NAT,
            // so we can pretend that NAT isn't occurring.
            //
            // Even if the tuples are compatible, it's not necessarily the
            // case that conntrack will find a corresponding connection for
            // the packet. That would require the payload tuple to belong to a
            // preexisting conntrack connection.
            (self.dst_addr() == payload.src_ip).then(|| {
                conntrack::PacketMetadata::new_from_icmp_error(
                    payload.src_ip,
                    payload.dst_ip,
                    payload.src_port,
                    payload.dst_port,
                    I::map_ip(payload.proto, |proto| proto.into(), |proto| proto.into()),
                )
            })
        } else {
            self.maybe_transport_packet().transport_packet_data().and_then(|transport_data| {
                let protocol =
                    I::map_ip(self.protocol()?, |proto| proto.into(), |proto| proto.into());
                match (protocol, &transport_data) {
                    (conntrack::TransportProtocol::Tcp, TransportPacketData::Tcp { .. }) => {}
                    // If the IP protocol is TCP, but we failed to parse the TCP header,
                    // we fall back to generic transport info. In that case, we do not want
                    // to track the packet, so we return `None`.
                    (conntrack::TransportProtocol::Tcp, TransportPacketData::Generic { .. }) => {
                        return None
                    }
                    (
                        conntrack::TransportProtocol::Udp
                        | conntrack::TransportProtocol::Icmp
                        | conntrack::TransportProtocol::Other(_),
                        TransportPacketData::Generic { .. },
                    ) => {}
                    (
                        conntrack::TransportProtocol::Udp
                        | conntrack::TransportProtocol::Icmp
                        | conntrack::TransportProtocol::Other(_),
                        TransportPacketData::Tcp { .. },
                    ) => unreachable!(
                        "non-TCP packet with TCP transport data: proto={protocol:?}, data={transport_data:?}"
                    ),
                }
                Some(conntrack::PacketMetadata::new(
                    self.src_addr(),
                    self.dst_addr(),
                    protocol,
                    transport_data,
                ))
            })
        }
    }
}

/// An `IpPacket` that allows to access raw packet contents.
// TODO(https://fxbug.dev/424212358): Currently this trait relies on
// PartialSerializer to access raw packet contents. It should be replaced with
// direct access to the packet contents when the packet is already serialized.
pub trait FilterIpPacket<I: FilterIpExt>: IpPacket<I> + NetworkPartialSerializer {}
impl<I: FilterIpExt, P: IpPacket<I> + NetworkPartialSerializer> FilterIpPacket<I> for P {}

/// A payload of an IP packet that may be a valid transport layer packet.
///
/// This trait exists to allow bubbling up the trait bound that a serializer
/// type implement `MaybeTransportPacket` from the IP socket layer to upper
/// layers, where it can be implemented separately on each concrete packet type
/// depending on whether it supports packet header inspection.
pub trait MaybeTransportPacket {
    /// Optionally returns a type that provides access to this transport-layer
    /// packet.
    fn transport_packet_data(&self) -> Option<TransportPacketData>;
}

/// A payload of an IP packet that may be a valid modifiable transport layer
/// packet.
///
/// This trait exists to allow bubbling up the trait bound that a serializer
/// type implement `MaybeTransportPacketMut` from the IP socket layer to upper
/// layers, where it can be implemented separately on each concrete packet type
/// depending on whether it supports packet header modification.
pub trait MaybeTransportPacketMut<I: IpExt> {
    /// The type that provides access to transport-layer header modification, if
    /// this is indeed a valid transport packet.
    type TransportPacketMut<'a>: TransportPacketMut<I>
    where
        Self: 'a;

    /// Optionally returns a type that provides mutable access to this
    /// transport-layer packet.
    fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>>;
}

/// An equivalent of [`MaybeTransportPacketMut`] that yields dynamic references
/// to the inner transport packet.
///
/// This is hand-rolled for each implementer of `MaybeTransportPacketMut`
/// because we don't quite have a trait to blanket impl this automatically for
/// things that need it.
///
/// Perhaps after unsize is stabilized this can be blanket implemented. See
/// https://github.com/rust-lang/rust/issues/18598.
pub trait DynamicMaybeTransportPacketMut<I: IpExt> {
    fn dyn_transport_packet_mut(&mut self) -> Option<&mut dyn TransportPacketMut<I>>;
}

/// A payload of an ICMP error packet that may contain an IP packet.
///
/// See also the note on [`MaybeTransportPacket`].
pub trait MaybeIcmpErrorPayload<I: IpExt> {
    /// Optionally returns a type that provides access to the payload of this
    /// ICMP error.
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>>;
}

/// A payload of an IP packet that may be a valid modifiable ICMP error message
/// (i.e., one that contains the prefix of an IP packet in its payload).
pub trait MaybeIcmpErrorMut<I: FilterIpExt> {
    type IcmpErrorMut<'a>: IcmpErrorMut<I>
    where
        Self: 'a;

    fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>>;
}

/// An equivalent of [`MaybeIcmpErrorMut`] that yields dynamic references
/// to the inner ICMP packet.
///
/// This is hand-rolled for each implementer of `MaybeIcmpErrorMut`
/// because we don't quite have a trait to blanket impl this automatically for
/// things that need it.
///
/// Perhaps after unsize is stabilized this can be blanket implemented. See
/// https://github.com/rust-lang/rust/issues/18598.
pub trait DynamicMaybeIcmpErrorMut<I: IpExt> {
    fn dyn_icmp_error_mut(&mut self) -> Option<&mut dyn DynamicIcmpErrorMut<I>>;
}

/// A serializer that may also be a valid transport layer packet.
pub trait TransportPacketSerializer<I: FilterIpExt>:
    NetworkSerializer
    + NetworkPartialSerializer
    + MaybeTransportPacket
    + MaybeTransportPacketMut<I>
    + MaybeIcmpErrorPayload<I>
    + MaybeIcmpErrorMut<I>
{
}

impl<I, S> TransportPacketSerializer<I> for S
where
    I: FilterIpExt,
    S: NetworkSerializer
        + NetworkPartialSerializer
        + MaybeTransportPacket
        + MaybeTransportPacketMut<I>
        + MaybeIcmpErrorPayload<I>
        + MaybeIcmpErrorMut<I>,
{
}

/// A trait allowing transport serializers to be put behind a dyn reference.
///
/// This is dynamic-dispatch equivalent of [`TransportPacketSerializer`]. Used
/// in conjunction with [`DynTransportSerializer`] it allows dynamic dispatch
/// for slow-path protocols.
pub trait DynamicTransportSerializer<I: FilterIpExt>:
    DynamicNetworkSerializer
    + DynamicNetworkPartialSerializer
    + MaybeTransportPacket
    + DynamicMaybeTransportPacketMut<I>
    + DynamicMaybeIcmpErrorMut<I>
    + MaybeIcmpErrorPayload<I>
{
}

impl<O, I> DynamicTransportSerializer<I> for O
where
    I: FilterIpExt,
    O: TransportPacketSerializer<I>
        + DynamicMaybeTransportPacketMut<I>
        + DynamicMaybeIcmpErrorMut<I>,
{
}

/// A concrete type around a dynamic reference to a
/// [`DynamicTransportSerializer`].
pub struct DynTransportSerializer<'a, I: FilterIpExt>(&'a mut dyn DynamicTransportSerializer<I>);

impl<'a, I: FilterIpExt> DynTransportSerializer<'a, I> {
    /// Creates a new [`DynTransportSerializer`] with a dynamic mutable borrow
    /// to a serializer.
    pub fn new(inner: &'a mut dyn DynamicTransportSerializer<I>) -> Self {
        Self(inner)
    }
}

impl<I: FilterIpExt> Serializer<NetworkSerializationContext> for DynTransportSerializer<'_, I> {
    type Buffer = EmptyBuf;

    fn serialize<B: GrowBufferMut, P: BufferProvider<Self::Buffer, B>>(
        self,
        context: &mut NetworkSerializationContext,
        constraints: PacketConstraints,
        provider: P,
    ) -> Result<B, (SerializeError<P::Error>, Self)> {
        match DynSerializer::new_dyn(self.0).serialize(context, constraints, provider) {
            Ok(r) => Ok(r),
            Err((e, _)) => Err((e, self)),
        }
    }

    fn serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
        &self,
        context: &mut NetworkSerializationContext,
        outer: PacketConstraints,
        alloc: A,
    ) -> Result<B, SerializeError<A::Error>> {
        DynSerializer::new_dyn(self.0).serialize_new_buf(context, outer, alloc)
    }
}

impl<'a, I: FilterIpExt> NestableSerializer for DynTransportSerializer<'a, I> {}

impl<'a, I: FilterIpExt> PartialSerializer<NetworkSerializationContext>
    for DynTransportSerializer<'a, I>
{
    fn partial_serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
        &self,
        context: &mut NetworkSerializationContext,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<(B, usize), SerializeError<A::Error>> {
        DynPartialSerializer::new_dyn(self.0).partial_serialize_new_buf(context, constraints, alloc)
    }
}

impl<'a, I: FilterIpExt> MaybeTransportPacket for DynTransportSerializer<'a, I> {
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        (*self.0).transport_packet_data()
    }
}

impl<'a, I: FilterIpExt> MaybeIcmpErrorPayload<I> for DynTransportSerializer<'a, I> {
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
        (*self.0).icmp_error_payload()
    }
}

impl<'a, I: FilterIpExt> MaybeIcmpErrorMut<I> for DynTransportSerializer<'a, I> {
    type IcmpErrorMut<'b>
        = &'b mut dyn DynamicIcmpErrorMut<I>
    where
        Self: 'b;

    fn icmp_error_mut(&mut self) -> Option<Self::IcmpErrorMut<'_>> {
        (*self.0).dyn_icmp_error_mut()
    }
}

impl<'a, I: FilterIpExt> MaybeTransportPacketMut<I> for DynTransportSerializer<'a, I> {
    type TransportPacketMut<'b>
        = &'b mut dyn TransportPacketMut<I>
    where
        Self: 'b;

    fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
        (*self.0).dyn_transport_packet_mut()
    }
}

impl<T: ?Sized> MaybeTransportPacket for &T
where
    T: MaybeTransportPacket,
{
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        (**self).transport_packet_data()
    }
}

impl<T: ?Sized, I: IpExt> MaybeIcmpErrorPayload<I> for &T
where
    T: MaybeIcmpErrorPayload<I>,
{
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
        (**self).icmp_error_payload()
    }
}

impl<T: ?Sized> MaybeTransportPacket for &mut T
where
    T: MaybeTransportPacket,
{
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        (**self).transport_packet_data()
    }
}

impl<I: IpExt, T: ?Sized> MaybeTransportPacketMut<I> for &mut T
where
    T: MaybeTransportPacketMut<I>,
{
    type TransportPacketMut<'a>
        = T::TransportPacketMut<'a>
    where
        Self: 'a;

    fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
        (**self).transport_packet_mut()
    }
}

impl<I: FilterIpExt, T: ?Sized> MaybeIcmpErrorMut<I> for &mut T
where
    T: MaybeIcmpErrorMut<I>,
{
    type IcmpErrorMut<'a>
        = T::IcmpErrorMut<'a>
    where
        Self: 'a;

    fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
        (**self).icmp_error_mut()
    }
}

impl<I: FilterIpExt, T: ?Sized> IcmpErrorMut<I> for &mut T
where
    T: IcmpErrorMut<I>,
{
    type InnerPacket<'a>
        = T::InnerPacket<'a>
    where
        Self: 'a;

    fn recalculate_checksum(&mut self) -> bool {
        (**self).recalculate_checksum()
    }

    fn inner_packet<'a>(&'a mut self) -> Option<Self::InnerPacket<'a>> {
        (**self).inner_packet()
    }
}

impl<I: IpExt, T: TransportPacketMut<I>> MaybeTransportPacketMut<I> for Option<T> {
    type TransportPacketMut<'a>
        = &'a mut T
    where
        Self: 'a;

    fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
        self.as_mut()
    }
}

impl<I: FilterIpExt, T> MaybeIcmpErrorMut<I> for Option<T>
where
    T: IcmpErrorMut<I>,
{
    type IcmpErrorMut<'a>
        = &'a mut T
    where
        Self: 'a;

    fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
        self.as_mut()
    }
}

/// A concrete enum to hold all of the transport packet data that could possibly be usefully
/// extracted from a packet.
#[derive(Debug, Clone, GenericOverIp, PartialEq, Eq)]
#[generic_over_ip()]
pub enum TransportPacketData {
    Tcp { src_port: u16, dst_port: u16, segment: SegmentHeader, payload_len: usize },
    Generic { src_port: u16, dst_port: u16 },
}

impl TransportPacketData {
    pub fn src_port(&self) -> u16 {
        match self {
            TransportPacketData::Tcp { src_port, .. }
            | TransportPacketData::Generic { src_port, .. } => *src_port,
        }
    }

    pub fn dst_port(&self) -> u16 {
        match self {
            TransportPacketData::Tcp { dst_port, .. }
            | TransportPacketData::Generic { dst_port, .. } => *dst_port,
        }
    }

    pub fn tcp_segment_and_len(&self) -> Option<(&SegmentHeader, usize)> {
        match self {
            TransportPacketData::Tcp { segment, payload_len, .. } => Some((&segment, *payload_len)),
            TransportPacketData::Generic { .. } => None,
        }
    }

    fn parse_in_ip_packet<I: IpExt, B: ParseBuffer>(
        src_ip: I::Addr,
        dst_ip: I::Addr,
        proto: I::Proto,
        body: B,
    ) -> Option<TransportPacketData> {
        I::map_ip(
            (src_ip, dst_ip, proto, IpInvariant(body)),
            |(src_ip, dst_ip, proto, IpInvariant(body))| {
                parse_transport_header_in_ipv4_packet(src_ip, dst_ip, proto, body)
            },
            |(src_ip, dst_ip, proto, IpInvariant(body))| {
                parse_transport_header_in_ipv6_packet(src_ip, dst_ip, proto, body)
            },
        )
    }
}

/// A transport layer packet that provides header modification.
//
// TODO(https://fxbug.dev/341128580): make this trait more expressive for the
// differences between transport protocols.
pub trait TransportPacketMut<I: IpExt> {
    /// Set the source port or identifier of the packet.
    fn set_src_port(&mut self, port: NonZeroU16);

    /// Set the destination port or identifier of the packet.
    fn set_dst_port(&mut self, port: NonZeroU16);

    /// Update the source IP address in the pseudo header.
    fn update_pseudo_header_src_addr(&mut self, old: I::Addr, new: I::Addr);

    /// Update the destination IP address in the pseudo header.
    fn update_pseudo_header_dst_addr(&mut self, old: I::Addr, new: I::Addr);
}

/// An ICMP error packet that provides mutable access to the contained IP
/// packet.
pub trait IcmpErrorMut<I: FilterIpExt> {
    type InnerPacket<'a>: IpPacket<I>
    where
        Self: 'a;

    /// Fully recalculate the checksum of this ICMP packet.
    ///
    /// Returns whether the checksum was successfully written.
    ///
    /// Must be called after modifying the IP packet contained within this ICMP
    /// error to ensure the checksum stays correct.
    fn recalculate_checksum(&mut self) -> bool;

    /// Returns an [`IpPacket`] of the packet contained within this error, if
    /// one is present.
    fn inner_packet<'a>(&'a mut self) -> Option<Self::InnerPacket<'a>>;
}

/// An equivalent of [`IcmpErrorMut`] that provides a `dyn-compatible` API for
/// ICMP errors.
///
/// This has the same shape as [`IcmpErrorMut`] except for the associated type,
/// forcing the inner packet type to something that is workable for all
/// implementations.
pub trait DynamicIcmpErrorMut<I: FilterIpExt> {
    fn dyn_recalculate_checksum(&mut self) -> bool;
    fn dyn_inner_packet(&mut self) -> Option<I::FilterIpPacketRaw<&mut [u8]>>;
}

impl<'a, I: FilterIpExt> IcmpErrorMut<I> for dyn DynamicIcmpErrorMut<I> + 'a {
    type InnerPacket<'b>
        = I::FilterIpPacketRaw<&'b mut [u8]>
    where
        Self: 'b;

    fn recalculate_checksum(&mut self) -> bool {
        self.dyn_recalculate_checksum()
    }
    fn inner_packet<'b>(&'b mut self) -> Option<Self::InnerPacket<'b>> {
        self.dyn_inner_packet()
    }
}

impl<B: SplitByteSliceMut> IpPacket<Ipv4> for Ipv4Packet<B> {
    type TransportPacket<'a>
        = &'a Self
    where
        Self: 'a;
    type TransportPacketMut<'a>
        = Option<ParsedTransportHeaderMut<'a, Ipv4>>
    where
        B: 'a;
    type IcmpError<'a>
        = &'a Self
    where
        Self: 'a;
    type IcmpErrorMut<'a>
        = Option<ParsedIcmpErrorMut<'a, Ipv4>>
    where
        B: 'a;

    fn src_addr(&self) -> Ipv4Addr {
        self.src_ip()
    }

    fn set_src_addr(&mut self, addr: Ipv4Addr) {
        let old = self.src_addr();
        if let Some(packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_src_addr(old, addr);
        }

        self.set_src_ip_and_update_checksum(addr);
    }

    fn dst_addr(&self) -> Ipv4Addr {
        self.dst_ip()
    }

    fn set_dst_addr(&mut self, addr: Ipv4Addr) {
        let old = self.dst_addr();
        if let Some(packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_dst_addr(old, addr);
        }

        self.set_dst_ip_and_update_checksum(addr);
    }

    fn protocol(&self) -> Option<Ipv4Proto> {
        Some(self.proto())
    }

    fn maybe_transport_packet(&self) -> Self::TransportPacket<'_> {
        self
    }

    fn transport_packet_mut(&mut self) -> Self::TransportPacketMut<'_> {
        ParsedTransportHeaderMut::parse_in_ipv4_packet(
            self.proto(),
            SliceBufViewMut::new(self.body_mut()),
        )
    }

    fn maybe_icmp_error<'a>(&'a self) -> Self::IcmpError<'a> {
        self
    }

    fn icmp_error_mut<'a>(&'a mut self) -> Self::IcmpErrorMut<'a> {
        ParsedIcmpErrorMut::parse_in_ipv4_packet(
            self.src_addr(),
            self.dst_addr(),
            self.proto(),
            SliceBufViewMut::new(self.body_mut()),
        )
    }
}

impl<B: SplitByteSlice> MaybeTransportPacket for Ipv4Packet<B> {
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        parse_transport_header_in_ipv4_packet(
            self.src_ip(),
            self.dst_ip(),
            self.proto(),
            self.body(),
        )
    }
}

impl<B: SplitByteSlice> MaybeIcmpErrorPayload<Ipv4> for Ipv4Packet<B> {
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<Ipv4>> {
        ParsedIcmpErrorPayload::parse_in_outer_ipv4_packet(self.proto(), Buf::new(self.body(), ..))
    }
}

impl<B: SplitByteSliceMut> IpPacket<Ipv4> for Ipv4PacketRaw<B> {
    type TransportPacket<'a>
        = &'a Self
    where
        Self: 'a;
    type TransportPacketMut<'a>
        = Option<ParsedTransportHeaderMut<'a, Ipv4>>
    where
        B: 'a;
    type IcmpError<'a>
        = &'a Self
    where
        Self: 'a;
    type IcmpErrorMut<'a>
        = Option<ParsedIcmpErrorMut<'a, Ipv4>>
    where
        B: 'a;

    fn src_addr(&self) -> Ipv4Addr {
        self.src_ip()
    }

    fn set_src_addr(&mut self, addr: Ipv4Addr) {
        let old = self.src_ip();
        if let Some(packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_src_addr(old, addr);
        }

        self.set_src_ip_and_update_checksum(addr);
    }

    fn dst_addr(&self) -> Ipv4Addr {
        self.dst_ip()
    }

    fn set_dst_addr(&mut self, addr: Ipv4Addr) {
        let old = self.dst_ip();
        if let Some(packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_dst_addr(old, addr);
        }

        self.set_dst_ip_and_update_checksum(addr);
    }

    fn protocol(&self) -> Option<Ipv4Proto> {
        Some(self.proto())
    }

    fn maybe_transport_packet<'a>(&'a self) -> Self::TransportPacket<'a> {
        self
    }

    fn transport_packet_mut<'a>(&'a mut self) -> Self::TransportPacketMut<'a> {
        ParsedTransportHeaderMut::parse_in_ipv4_packet(
            self.proto(),
            SliceBufViewMut::new(self.body_mut()),
        )
    }

    fn maybe_icmp_error<'a>(&'a self) -> Self::IcmpError<'a> {
        self
    }

    fn icmp_error_mut<'a>(&'a mut self) -> Self::IcmpErrorMut<'a> {
        ParsedIcmpErrorMut::parse_in_ipv4_packet(
            self.src_addr(),
            self.dst_addr(),
            self.proto(),
            SliceBufViewMut::new(self.body_mut()),
        )
    }
}

impl<B: SplitByteSlice> MaybeTransportPacket for Ipv4PacketRaw<B> {
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        parse_transport_header_in_ipv4_packet(
            self.src_ip(),
            self.dst_ip(),
            self.proto(),
            // We don't particularly care whether we have the full packet, since
            // we're only looking at transport headers.
            self.body().into_inner(),
        )
    }
}

impl<B: SplitByteSlice> MaybeIcmpErrorPayload<Ipv4> for Ipv4PacketRaw<B> {
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<Ipv4>> {
        ParsedIcmpErrorPayload::parse_in_outer_ipv4_packet(
            self.proto(),
            // We don't particularly care whether we have the full packet, since
            // we're only looking at transport headers.
            Buf::new(self.body().into_inner(), ..),
        )
    }
}

impl<B: SplitByteSliceMut> IpPacket<Ipv6> for Ipv6Packet<B> {
    type TransportPacket<'a>
        = &'a Self
    where
        Self: 'a;
    type TransportPacketMut<'a>
        = Option<ParsedTransportHeaderMut<'a, Ipv6>>
    where
        B: 'a;
    type IcmpError<'a>
        = &'a Self
    where
        Self: 'a;
    type IcmpErrorMut<'a>
        = Option<ParsedIcmpErrorMut<'a, Ipv6>>
    where
        B: 'a;

    fn src_addr(&self) -> Ipv6Addr {
        self.src_ip()
    }

    fn set_src_addr(&mut self, addr: Ipv6Addr) {
        let old = self.src_addr();
        if let Some(packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_src_addr(old, addr);
        }

        self.set_src_ip(addr);
    }

    fn dst_addr(&self) -> Ipv6Addr {
        self.dst_ip()
    }

    fn set_dst_addr(&mut self, addr: Ipv6Addr) {
        let old = self.dst_addr();
        if let Some(packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_dst_addr(old, addr);
        }

        self.set_dst_ip(addr);
    }

    fn protocol(&self) -> Option<Ipv6Proto> {
        Some(self.proto())
    }

    fn maybe_transport_packet(&self) -> Self::TransportPacket<'_> {
        self
    }

    fn transport_packet_mut(&mut self) -> Self::TransportPacketMut<'_> {
        ParsedTransportHeaderMut::parse_in_ipv6_packet(
            self.proto(),
            SliceBufViewMut::new(self.body_mut()),
        )
    }

    fn maybe_icmp_error<'a>(&'a self) -> Self::IcmpError<'a> {
        self
    }

    fn icmp_error_mut<'a>(&'a mut self) -> Self::IcmpErrorMut<'a> {
        ParsedIcmpErrorMut::parse_in_ipv6_packet(
            self.src_addr(),
            self.dst_addr(),
            self.proto(),
            SliceBufViewMut::new(self.body_mut()),
        )
    }
}

impl<B: SplitByteSlice> MaybeTransportPacket for Ipv6Packet<B> {
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        parse_transport_header_in_ipv6_packet(
            self.src_ip(),
            self.dst_ip(),
            self.proto(),
            self.body(),
        )
    }
}

impl<B: SplitByteSlice> MaybeIcmpErrorPayload<Ipv6> for Ipv6Packet<B> {
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<Ipv6>> {
        ParsedIcmpErrorPayload::parse_in_outer_ipv6_packet(self.proto(), Buf::new(self.body(), ..))
    }
}

impl<B: SplitByteSliceMut> IpPacket<Ipv6> for Ipv6PacketRaw<B> {
    type TransportPacket<'a>
        = &'a Self
    where
        Self: 'a;
    type TransportPacketMut<'a>
        = Option<ParsedTransportHeaderMut<'a, Ipv6>>
    where
        B: 'a;
    type IcmpError<'a>
        = &'a Self
    where
        Self: 'a;
    type IcmpErrorMut<'a>
        = Option<ParsedIcmpErrorMut<'a, Ipv6>>
    where
        B: 'a;

    fn src_addr(&self) -> Ipv6Addr {
        self.src_ip()
    }

    fn set_src_addr(&mut self, addr: Ipv6Addr) {
        let old = self.src_ip();
        if let Some(packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_src_addr(old, addr);
        }

        self.set_src_ip(addr);
    }

    fn dst_addr(&self) -> Ipv6Addr {
        self.dst_ip()
    }

    fn set_dst_addr(&mut self, addr: Ipv6Addr) {
        let old = self.dst_ip();
        if let Some(packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_dst_addr(old, addr);
        }

        self.set_dst_ip(addr);
    }

    fn protocol(&self) -> Option<Ipv6Proto> {
        self.proto().ok()
    }

    fn maybe_transport_packet<'a>(&'a self) -> Self::TransportPacket<'a> {
        self
    }

    fn transport_packet_mut(&mut self) -> Self::TransportPacketMut<'_> {
        let proto = self.proto().ok()?;
        let body = self.body_mut()?;
        ParsedTransportHeaderMut::parse_in_ipv6_packet(proto, SliceBufViewMut::new(body))
    }

    fn maybe_icmp_error<'a>(&'a self) -> Self::IcmpError<'a> {
        self
    }

    fn icmp_error_mut<'a>(&'a mut self) -> Self::IcmpErrorMut<'a> {
        let src_addr = self.src_addr();
        let dst_addr = self.dst_addr();
        let proto = self.proto().ok()?;
        let body = self.body_mut()?;

        ParsedIcmpErrorMut::parse_in_ipv6_packet(
            src_addr,
            dst_addr,
            proto,
            SliceBufViewMut::new(body),
        )
    }
}

impl<B: SplitByteSlice> MaybeTransportPacket for Ipv6PacketRaw<B> {
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        let (body, proto) = self.body_proto().ok()?;
        parse_transport_header_in_ipv6_packet(
            self.src_ip(),
            self.dst_ip(),
            proto,
            body.into_inner(),
        )
    }
}

impl<B: SplitByteSlice> MaybeIcmpErrorPayload<Ipv6> for Ipv6PacketRaw<B> {
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<Ipv6>> {
        let (body, proto) = self.body_proto().ok()?;
        ParsedIcmpErrorPayload::parse_in_outer_ipv6_packet(proto, Buf::new(body.into_inner(), ..))
    }
}

/// An outgoing IP packet that has not yet been wrapped into an outer serializer
/// type.
#[derive(Debug, PartialEq, GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct TxPacket<'a, I: IpExt, S> {
    src_addr: I::Addr,
    dst_addr: I::Addr,
    protocol: I::Proto,
    serializer: &'a mut S,
}

impl<'a, I: IpExt, S> TxPacket<'a, I, S> {
    /// Create a new [`TxPacket`] from its IP header fields and payload.
    pub fn new(
        src_addr: I::Addr,
        dst_addr: I::Addr,
        protocol: I::Proto,
        serializer: &'a mut S,
    ) -> Self {
        Self { src_addr, dst_addr, protocol, serializer }
    }

    /// The source IP address of the packet.
    pub fn src_addr(&self) -> I::Addr {
        self.src_addr
    }

    /// The destination IP address of the packet.
    pub fn dst_addr(&self) -> I::Addr {
        self.dst_addr
    }
}

impl<I: FilterIpExt, S: TransportPacketSerializer<I>> IpPacket<I> for TxPacket<'_, I, S> {
    type TransportPacket<'a>
        = &'a S
    where
        Self: 'a;
    type TransportPacketMut<'a>
        = &'a mut S
    where
        Self: 'a;
    type IcmpError<'a>
        = &'a S
    where
        Self: 'a;
    type IcmpErrorMut<'a>
        = &'a mut S
    where
        Self: 'a;

    fn src_addr(&self) -> I::Addr {
        self.src_addr
    }

    fn set_src_addr(&mut self, addr: I::Addr) {
        let old = core::mem::replace(&mut self.src_addr, addr);
        if let Some(mut packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_src_addr(old, addr);
        }
    }

    fn dst_addr(&self) -> I::Addr {
        self.dst_addr
    }

    fn set_dst_addr(&mut self, addr: I::Addr) {
        let old = core::mem::replace(&mut self.dst_addr, addr);
        if let Some(mut packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_dst_addr(old, addr);
        }
    }

    fn protocol(&self) -> Option<I::Proto> {
        Some(self.protocol)
    }

    fn maybe_transport_packet(&self) -> Self::TransportPacket<'_> {
        self.serializer
    }

    fn transport_packet_mut(&mut self) -> Self::TransportPacketMut<'_> {
        self.serializer
    }

    fn maybe_icmp_error<'a>(&'a self) -> Self::IcmpError<'a> {
        self.serializer
    }

    fn icmp_error_mut<'a>(&'a mut self) -> Self::IcmpErrorMut<'a> {
        self.serializer
    }
}

/// Implements `PartialSerializer` for a reference to a `PartialSerializer`
/// implementation. It's not possible to provide a blanket implementation for
/// references directly (i.e. for `&S`) since it would conflict with the
/// implementation for `FragmentedBuffer`.
pub struct PartialSerializeRef<'a, S> {
    reference: &'a S,
}

impl<'a, C, S> PartialSerializer<C> for PartialSerializeRef<'a, S>
where
    C: SerializationContext,
    S: PartialSerializer<C>,
{
    fn partial_serialize<B: GrowBufferMut + ContiguousBuffer, A: LayoutBufferAlloc<B>>(
        &self,
        context: &mut C,
        alloc: A,
    ) -> Result<PartialSerializeResult<'_, B>, SerializeError<A::Error>> {
        self.reference.partial_serialize(context, alloc)
    }

    fn partial_serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
        &self,
        context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<(B, usize), SerializeError<A::Error>> {
        self.reference.partial_serialize_new_buf(context, constraints, alloc)
    }
}

/// Value used in place of TTL in a partially-serialized TxPacket.
const TX_PACKET_NO_TTL: u8 = 0;

/// `TxPacket` is used for eBPF CGROUP_EGRESS filters. At that level the packet
/// is not fragmented yet, so we don't have a final packet serializer, but the
/// eBPF filters want to see a serialized packet. We provide `PartialSerialize`,
/// which allows to serialize just the packet headers - that's enough for most
/// eBPF programs. TTL is not known here, so the field is set to 64.
impl<I: FilterIpExt, S: TransportPacketSerializer<I> + NetworkPartialSerializer>
    PartialSerializer<NetworkSerializationContext> for TxPacket<'_, I, S>
{
    fn partial_serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
        &self,
        context: &mut NetworkSerializationContext,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<(B, usize), SerializeError<A::Error>> {
        let packet_builder =
            I::PacketBuilder::new(self.src_addr, self.dst_addr, TX_PACKET_NO_TTL, self.protocol);
        packet_builder
            .wrap_body(PartialSerializeRef { reference: self.serializer })
            .partial_serialize_new_buf(context, constraints, alloc)
    }
}

/// An incoming IP packet that is being forwarded.
#[derive(Debug, PartialEq, GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct ForwardedPacket<I: IpExt, B> {
    src_addr: I::Addr,
    dst_addr: I::Addr,
    protocol: I::Proto,
    transport_header_offset: usize,
    buffer: B,
    /// Whether this packet was reassembled from fragments.
    reassembled: bool,
}

impl<I: IpExt, B: BufferMut> ForwardedPacket<I, B> {
    /// Create a new [`ForwardedPacket`] from its IP header fields and payload.
    ///
    /// `meta` is used to revert `buffer` back to the IP header for further
    /// serialization, and to mark where the transport header starts in
    /// `buffer`. It _must_ have originated from a previously parsed IP packet
    /// on `buffer`.
    pub fn new(
        src_addr: I::Addr,
        dst_addr: I::Addr,
        protocol: I::Proto,
        meta: ParseMetadata,
        mut buffer: B,
        reassembled: bool,
    ) -> Self {
        let transport_header_offset = meta.header_len();
        buffer.undo_parse(meta);
        Self { src_addr, dst_addr, protocol, transport_header_offset, buffer, reassembled }
    }

    /// Returns whether this forwarded packet was reassembled from fragments.
    pub fn reassembled(&self) -> bool {
        self.reassembled
    }

    /// Discard the metadata carried by the [`ForwardedPacket`] and return the
    /// inner buffer.
    ///
    /// The returned buffer is guaranteed to contain a valid IP frame, the
    /// start of the buffer points at the start of the IP header.
    pub fn into_buffer(self) -> B {
        self.buffer
    }

    /// Returns a reference to the forwarded buffer.
    ///
    /// The returned reference is guaranteed to contain a valid IP frame, the
    /// start of the buffer points at the start of the IP header.
    pub fn buffer(&self) -> &B {
        &self.buffer
    }
}

impl<I: IpExt, B: BufferMut + NetworkSerializer> Serializer<NetworkSerializationContext>
    for ForwardedPacket<I, B>
{
    type Buffer = <B as Serializer<NetworkSerializationContext>>::Buffer;

    fn serialize<G: packet::GrowBufferMut, P: packet::BufferProvider<Self::Buffer, G>>(
        self,
        context: &mut NetworkSerializationContext,
        constraints: packet::PacketConstraints,
        provider: P,
    ) -> Result<G, (packet::SerializeError<P::Error>, Self)> {
        let Self { src_addr, dst_addr, protocol, transport_header_offset, buffer, reassembled } =
            self;
        buffer.serialize(context, constraints, provider).map_err(|(err, buffer)| {
            (
                err,
                Self { src_addr, dst_addr, protocol, transport_header_offset, buffer, reassembled },
            )
        })
    }

    fn serialize_new_buf<BB: GrowBufferMut, A: LayoutBufferAlloc<BB>>(
        &self,
        context: &mut NetworkSerializationContext,
        outer: packet::PacketConstraints,
        alloc: A,
    ) -> Result<BB, packet::SerializeError<A::Error>> {
        self.buffer.serialize_new_buf(context, outer, alloc)
    }
}

impl<I: IpExt, B: BufferMut + NetworkSerializer> NestableSerializer for ForwardedPacket<I, B> {}

impl<C: SerializationContext, I: IpExt, B: BufferMut> PartialSerializer<C>
    for ForwardedPacket<I, B>
{
    fn partial_serialize<BB: GrowBufferMut + ContiguousBuffer, A: LayoutBufferAlloc<BB>>(
        &self,
        _context: &mut C,
        _alloc: A,
    ) -> Result<PartialSerializeResult<'_, BB>, SerializeError<A::Error>> {
        Ok(PartialSerializeResult::Slice(self.buffer.as_ref()))
    }

    fn partial_serialize_new_buf<BB: GrowBufferMut, A: LayoutBufferAlloc<BB>>(
        &self,
        _context: &mut C,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<(BB, usize), SerializeError<A::Error>> {
        let bytes_to_copy = cmp::min(
            self.buffer.as_ref().len(),
            self.transport_header_offset + TRANSPORT_HEADER_MAX_SIZE,
        );
        let mut buffer = alloc.layout_alloc(constraints.header_len(), bytes_to_copy, 0)?;
        buffer.with_parts_mut(|_prefix, mut body, _suffix| {
            body.copy_from_slice(&self.buffer.as_ref()[..bytes_to_copy]);
        });
        Ok((buffer, self.buffer.as_ref().len()))
    }
}

impl<I: FilterIpExt, B: BufferMut> IpPacket<I> for ForwardedPacket<I, B> {
    type TransportPacket<'a>
        = &'a Self
    where
        Self: 'a;
    type TransportPacketMut<'a>
        = Option<ParsedTransportHeaderMut<'a, I>>
    where
        Self: 'a;
    type IcmpError<'a>
        = &'a Self
    where
        Self: 'a;

    type IcmpErrorMut<'a>
        = Option<ParsedIcmpErrorMut<'a, I>>
    where
        Self: 'a;

    fn src_addr(&self) -> I::Addr {
        self.src_addr
    }

    fn set_src_addr(&mut self, addr: I::Addr) {
        // Re-parse the IP header so we can modify it in place.
        I::map_ip::<_, ()>(
            (IpInvariant(self.buffer.as_mut()), addr),
            |(IpInvariant(buffer), addr)| {
                let mut packet = Ipv4PacketRaw::parse_mut(SliceBufViewMut::new(buffer), ())
                    .expect("ForwardedPacket must have been created from a valid IP packet");
                packet.set_src_ip_and_update_checksum(addr);
            },
            |(IpInvariant(buffer), addr)| {
                let mut packet = Ipv6PacketRaw::parse_mut(SliceBufViewMut::new(buffer), ())
                    .expect("ForwardedPacket must have been created from a valid IP packet");
                packet.set_src_ip(addr);
            },
        );

        let old = self.src_addr;
        if let Some(packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_src_addr(old, addr);
        }

        self.src_addr = addr;
    }

    fn dst_addr(&self) -> I::Addr {
        self.dst_addr
    }

    fn set_dst_addr(&mut self, addr: I::Addr) {
        // Re-parse the IP header so we can modify it in place.
        I::map_ip::<_, ()>(
            (IpInvariant(self.buffer.as_mut()), addr),
            |(IpInvariant(buffer), addr)| {
                let mut packet = Ipv4PacketRaw::parse_mut(SliceBufViewMut::new(buffer), ())
                    .expect("ForwardedPacket must have been created from a valid IP packet");
                packet.set_dst_ip_and_update_checksum(addr);
            },
            |(IpInvariant(buffer), addr)| {
                let mut packet = Ipv6PacketRaw::parse_mut(SliceBufViewMut::new(buffer), ())
                    .expect("ForwardedPacket must have been created from a valid IP packet");
                packet.set_dst_ip(addr);
            },
        );

        let old = self.dst_addr;
        if let Some(packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_dst_addr(old, addr);
        }

        self.dst_addr = addr;
    }

    fn protocol(&self) -> Option<I::Proto> {
        Some(self.protocol)
    }

    fn maybe_transport_packet(&self) -> Self::TransportPacket<'_> {
        self
    }

    fn transport_packet_mut(&mut self) -> Self::TransportPacketMut<'_> {
        let ForwardedPacket {
            src_addr: _,
            dst_addr: _,
            protocol,
            buffer,
            transport_header_offset,
            reassembled: _,
        } = self;
        ParsedTransportHeaderMut::<I>::parse_in_ip_packet(
            *protocol,
            SliceBufViewMut::new(&mut buffer.as_mut()[*transport_header_offset..]),
        )
    }

    fn maybe_icmp_error<'a>(&'a self) -> Self::IcmpError<'a> {
        self
    }

    fn icmp_error_mut<'a>(&'a mut self) -> Self::IcmpErrorMut<'a> {
        let ForwardedPacket {
            src_addr,
            dst_addr,
            protocol,
            buffer,
            transport_header_offset,
            reassembled: _,
        } = self;

        ParsedIcmpErrorMut::<I>::parse_in_ip_packet(
            *src_addr,
            *dst_addr,
            *protocol,
            SliceBufViewMut::new(&mut buffer.as_mut()[*transport_header_offset..]),
        )
    }
}

impl<I: IpExt, B: BufferMut> MaybeTransportPacket for ForwardedPacket<I, B> {
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        let ForwardedPacket {
            protocol,
            buffer,
            src_addr,
            dst_addr,
            transport_header_offset,
            reassembled: _,
        } = self;
        TransportPacketData::parse_in_ip_packet::<I, _>(
            *src_addr,
            *dst_addr,
            *protocol,
            Buf::new(&buffer.as_ref()[*transport_header_offset..], ..),
        )
    }
}

impl<I: IpExt, B: BufferMut> MaybeIcmpErrorPayload<I> for ForwardedPacket<I, B> {
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
        let Self {
            src_addr: _,
            dst_addr: _,
            protocol,
            transport_header_offset,
            buffer,
            reassembled: _,
        } = self;
        ParsedIcmpErrorPayload::parse_in_outer_ip_packet(
            *protocol,
            Buf::new(&buffer.as_ref()[*transport_header_offset..], ..),
        )
    }
}

impl<
    I: FilterIpExt,
    S: TransportPacketSerializer<I>,
    B: IpPacketBuilder<NetworkSerializationContext, I>,
> IpPacket<I> for Nested<S, B>
{
    type TransportPacket<'a>
        = &'a S
    where
        Self: 'a;
    type TransportPacketMut<'a>
        = &'a mut S
    where
        Self: 'a;
    type IcmpError<'a>
        = &'a S
    where
        Self: 'a;
    type IcmpErrorMut<'a>
        = &'a mut S
    where
        Self: 'a;

    fn src_addr(&self) -> I::Addr {
        self.outer().src_ip()
    }

    fn set_src_addr(&mut self, addr: I::Addr) {
        let old = self.outer().src_ip();
        self.outer_mut().set_src_ip(addr);
        if let Some(mut packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_src_addr(old, addr);
        }
    }

    fn dst_addr(&self) -> I::Addr {
        self.outer().dst_ip()
    }

    fn set_dst_addr(&mut self, addr: I::Addr) {
        let old = self.outer().dst_ip();
        self.outer_mut().set_dst_ip(addr);
        if let Some(mut packet) = self.transport_packet_mut().transport_packet_mut() {
            packet.update_pseudo_header_dst_addr(old, addr);
        }
    }

    fn protocol(&self) -> Option<I::Proto> {
        Some(self.outer().proto())
    }

    fn maybe_transport_packet(&self) -> Self::TransportPacket<'_> {
        self.inner()
    }

    fn transport_packet_mut(&mut self) -> Self::TransportPacketMut<'_> {
        self.inner_mut()
    }

    fn maybe_icmp_error<'a>(&'a self) -> Self::IcmpError<'a> {
        self.inner()
    }

    fn icmp_error_mut<'a>(&'a mut self) -> Self::IcmpErrorMut<'a> {
        self.inner_mut()
    }
}

impl<I: IpExt, T: ?Sized> TransportPacketMut<I> for &mut T
where
    T: TransportPacketMut<I>,
{
    fn set_src_port(&mut self, port: NonZeroU16) {
        (*self).set_src_port(port);
    }

    fn set_dst_port(&mut self, port: NonZeroU16) {
        (*self).set_dst_port(port);
    }

    fn update_pseudo_header_src_addr(&mut self, old: I::Addr, new: I::Addr) {
        (*self).update_pseudo_header_src_addr(old, new);
    }

    fn update_pseudo_header_dst_addr(&mut self, old: I::Addr, new: I::Addr) {
        (*self).update_pseudo_header_dst_addr(old, new);
    }
}

impl<I: FilterIpExt> IpPacket<I> for Never {
    type TransportPacket<'a>
        = Never
    where
        Self: 'a;
    type TransportPacketMut<'a>
        = Never
    where
        Self: 'a;
    type IcmpError<'a>
        = Never
    where
        Self: 'a;
    type IcmpErrorMut<'a>
        = Never
    where
        Self: 'a;

    fn src_addr(&self) -> I::Addr {
        match *self {}
    }

    fn set_src_addr(&mut self, _addr: I::Addr) {
        match *self {}
    }

    fn dst_addr(&self) -> I::Addr {
        match *self {}
    }

    fn protocol(&self) -> Option<I::Proto> {
        match *self {}
    }

    fn set_dst_addr(&mut self, _addr: I::Addr) {
        match *self {}
    }

    fn maybe_transport_packet<'a>(&'a self) -> Self::TransportPacket<'a> {
        match *self {}
    }

    fn transport_packet_mut<'a>(&'a mut self) -> Self::TransportPacketMut<'a> {
        match *self {}
    }

    fn maybe_icmp_error<'a>(&'a self) -> Self::IcmpError<'a> {
        match *self {}
    }

    fn icmp_error_mut<'a>(&'a mut self) -> Self::IcmpErrorMut<'a> {
        match *self {}
    }
}

impl MaybeTransportPacket for Never {
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        match *self {}
    }
}

impl<I: IpExt> MaybeTransportPacketMut<I> for Never {
    type TransportPacketMut<'a>
        = Never
    where
        Self: 'a;

    fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
        match *self {}
    }
}

impl<I: IpExt> TransportPacketMut<I> for Never {
    fn set_src_port(&mut self, _: NonZeroU16) {
        match *self {}
    }

    fn set_dst_port(&mut self, _: NonZeroU16) {
        match *self {}
    }

    fn update_pseudo_header_src_addr(&mut self, _: I::Addr, _: I::Addr) {
        match *self {}
    }

    fn update_pseudo_header_dst_addr(&mut self, _: I::Addr, _: I::Addr) {
        match *self {}
    }
}

impl<I: IpExt> MaybeIcmpErrorPayload<I> for Never {
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
        match *self {}
    }
}

impl<I: FilterIpExt> MaybeIcmpErrorMut<I> for Never {
    type IcmpErrorMut<'a>
        = Never
    where
        Self: 'a;

    fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
        match *self {}
    }
}

impl<I: FilterIpExt> IcmpErrorMut<I> for Never {
    type InnerPacket<'a>
        = Never
    where
        Self: 'a;

    fn inner_packet<'a>(&'a mut self) -> Option<Self::InnerPacket<'a>> {
        match *self {}
    }

    fn recalculate_checksum(&mut self) -> bool {
        match *self {}
    }
}

impl<A: IpAddress, Inner> MaybeTransportPacket for Nested<Inner, UdpPacketBuilder<A>> {
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        Some(TransportPacketData::Generic {
            src_port: self.outer().src_port().map_or(0, NonZeroU16::get),
            dst_port: self.outer().dst_port().map_or(0, NonZeroU16::get),
        })
    }
}

impl<I: IpExt, Inner> MaybeTransportPacketMut<I> for Nested<Inner, UdpPacketBuilder<I::Addr>> {
    type TransportPacketMut<'a>
        = &'a mut Self
    where
        Self: 'a;

    fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
        Some(self)
    }
}

impl<I: IpExt, Inner> TransportPacketMut<I> for Nested<Inner, UdpPacketBuilder<I::Addr>> {
    fn set_src_port(&mut self, port: NonZeroU16) {
        self.outer_mut().set_src_port(port.get());
    }

    fn set_dst_port(&mut self, port: NonZeroU16) {
        self.outer_mut().set_dst_port(port);
    }

    fn update_pseudo_header_src_addr(&mut self, _old: I::Addr, new: I::Addr) {
        self.outer_mut().set_src_ip(new);
    }

    fn update_pseudo_header_dst_addr(&mut self, _old: I::Addr, new: I::Addr) {
        self.outer_mut().set_dst_ip(new);
    }
}

impl<A: IpAddress, I: IpExt, Inner> MaybeIcmpErrorPayload<I>
    for Nested<Inner, UdpPacketBuilder<A>>
{
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
        None
    }
}

impl<A: IpAddress, I: FilterIpExt, Inner> MaybeIcmpErrorMut<I>
    for Nested<Inner, UdpPacketBuilder<A>>
{
    type IcmpErrorMut<'a>
        = Never
    where
        Self: 'a;

    fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
        None
    }
}

impl<'a, A: IpAddress, Inner: PayloadLen> MaybeTransportPacket
    for Nested<Inner, TcpSegmentBuilderWithOptions<A, TcpOptionsBuilder<'a>>>
{
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        Some(TransportPacketData::Tcp {
            src_port: self.outer().src_port().map_or(0, NonZeroU16::get),
            dst_port: self.outer().dst_port().map_or(0, NonZeroU16::get),
            segment: self.outer().try_into().ok()?,
            payload_len: self.inner().len(),
        })
    }
}

impl<I: IpExt, Outer, Inner> MaybeTransportPacketMut<I>
    for Nested<Inner, TcpSegmentBuilderWithOptions<I::Addr, Outer>>
{
    type TransportPacketMut<'a>
        = &'a mut Self
    where
        Self: 'a;

    fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
        Some(self)
    }
}

impl<I: IpExt, Outer, Inner> TransportPacketMut<I>
    for Nested<Inner, TcpSegmentBuilderWithOptions<I::Addr, Outer>>
{
    fn set_src_port(&mut self, port: NonZeroU16) {
        self.outer_mut().set_src_port(port);
    }

    fn set_dst_port(&mut self, port: NonZeroU16) {
        self.outer_mut().set_dst_port(port);
    }

    fn update_pseudo_header_src_addr(&mut self, _old: I::Addr, new: I::Addr) {
        self.outer_mut().set_src_ip(new);
    }

    fn update_pseudo_header_dst_addr(&mut self, _old: I::Addr, new: I::Addr) {
        self.outer_mut().set_dst_ip(new);
    }
}

impl<A: IpAddress, I: IpExt, Inner, O> MaybeIcmpErrorPayload<I>
    for Nested<Inner, TcpSegmentBuilderWithOptions<A, O>>
{
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
        None
    }
}

impl<A: IpAddress, I: FilterIpExt, Inner, O> MaybeIcmpErrorMut<I>
    for Nested<Inner, TcpSegmentBuilderWithOptions<A, O>>
{
    type IcmpErrorMut<'a>
        = Never
    where
        Self: 'a;

    fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
        None
    }
}

impl<I: IpExt, Inner, M: IcmpMessage<I>> MaybeTransportPacket
    for Nested<Inner, IcmpPacketBuilder<I, M>>
{
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        self.outer().message().transport_packet_data()
    }
}

impl<I: IpExt, Inner, M: IcmpMessage<I>> MaybeTransportPacketMut<I>
    for Nested<Inner, IcmpPacketBuilder<I, M>>
{
    type TransportPacketMut<'a>
        = &'a mut IcmpPacketBuilder<I, M>
    where
        M: 'a,
        Inner: 'a;

    fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
        Some(self.outer_mut())
    }
}

impl<I: IpExt, Inner, M: IcmpMessage<I>> DynamicMaybeTransportPacketMut<I>
    for Nested<Inner, IcmpPacketBuilder<I, M>>
{
    fn dyn_transport_packet_mut(&mut self) -> Option<&mut dyn TransportPacketMut<I>> {
        MaybeTransportPacketMut::transport_packet_mut(self).map(|x| x as _)
    }
}

impl<I: IpExt, M: IcmpMessage<I>> TransportPacketMut<I> for IcmpPacketBuilder<I, M> {
    fn set_src_port(&mut self, id: NonZeroU16) {
        if M::IS_REWRITABLE {
            let _: u16 = self.message_mut().update_icmp_id(id.get());
        }
    }

    fn set_dst_port(&mut self, id: NonZeroU16) {
        if M::IS_REWRITABLE {
            let _: u16 = self.message_mut().update_icmp_id(id.get());
        }
    }

    fn update_pseudo_header_src_addr(&mut self, _old: I::Addr, new: I::Addr) {
        self.set_src_ip(new);
    }

    fn update_pseudo_header_dst_addr(&mut self, _old: I::Addr, new: I::Addr) {
        self.set_dst_ip(new);
    }
}

impl<Inner, I: IpExt> MaybeIcmpErrorPayload<I>
    for Nested<Inner, IcmpPacketBuilder<I, IcmpEchoRequest>>
{
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
        None
    }
}

impl<Inner, I: FilterIpExt> MaybeIcmpErrorMut<I>
    for Nested<Inner, IcmpPacketBuilder<I, IcmpEchoRequest>>
{
    type IcmpErrorMut<'a>
        = Never
    where
        Self: 'a;

    fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
        None
    }
}

impl<Inner, I: FilterIpExt> DynamicMaybeIcmpErrorMut<I>
    for Nested<Inner, IcmpPacketBuilder<I, IcmpEchoRequest>>
{
    fn dyn_icmp_error_mut(&mut self) -> Option<&mut dyn DynamicIcmpErrorMut<I>> {
        MaybeIcmpErrorMut::<I>::icmp_error_mut(self).map(|x| match x {})
    }
}

impl<Inner, I: IpExt> MaybeIcmpErrorPayload<I>
    for Nested<Inner, IcmpPacketBuilder<I, IcmpEchoReply>>
{
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
        None
    }
}

impl<Inner, I: FilterIpExt> MaybeIcmpErrorMut<I>
    for Nested<Inner, IcmpPacketBuilder<I, IcmpEchoReply>>
{
    type IcmpErrorMut<'a>
        = Never
    where
        Self: 'a;

    fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
        None
    }
}

impl<Inner, I: FilterIpExt> DynamicMaybeIcmpErrorMut<I>
    for Nested<Inner, IcmpPacketBuilder<I, IcmpEchoReply>>
{
    fn dyn_icmp_error_mut(&mut self) -> Option<&mut dyn DynamicIcmpErrorMut<I>> {
        MaybeIcmpErrorMut::<I>::icmp_error_mut(self).map(|x| match x {})
    }
}

/// An ICMP message type that may allow for transport-layer packet inspection.
pub trait IcmpMessage<I: IpExt>: icmp::IcmpMessage<I> + MaybeTransportPacket {
    /// Whether this ICMP message supports rewriting the ID.
    const IS_REWRITABLE: bool;

    /// The same as [`IcmpMessage::IS_REWRITABLE`], but for when you have an
    /// object, rather than a type.
    fn is_rewritable(&self) -> bool {
        Self::IS_REWRITABLE
    }

    /// Sets the ICMP ID for the message, returning the previous value.
    ///
    /// The ICMP ID is both the *src* AND *dst* ports for conntrack entries.
    fn update_icmp_id(&mut self, id: u16) -> u16;
}

// TODO(https://fxbug.dev/341128580): connection tracking will probably want to
// special case ICMP echo packets to ensure that a new connection is only ever
// created from an echo request, and not an echo response. We need to provide a
// way for conntrack to differentiate between the two.
impl MaybeTransportPacket for IcmpEchoReply {
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        Some(TransportPacketData::Generic { src_port: self.id(), dst_port: self.id() })
    }
}

impl<I: IpExt> IcmpMessage<I> for IcmpEchoReply {
    const IS_REWRITABLE: bool = true;

    fn update_icmp_id(&mut self, id: u16) -> u16 {
        let old = self.id();
        self.set_id(id);
        old
    }
}

// TODO(https://fxbug.dev/341128580): connection tracking will probably want to
// special case ICMP echo packets to ensure that a new connection is only ever
// created from an echo request, and not an echo response. We need to provide a
// way for conntrack to differentiate between the two.
impl MaybeTransportPacket for IcmpEchoRequest {
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        Some(TransportPacketData::Generic { src_port: self.id(), dst_port: self.id() })
    }
}

impl<I: IpExt> IcmpMessage<I> for IcmpEchoRequest {
    const IS_REWRITABLE: bool = true;

    fn update_icmp_id(&mut self, id: u16) -> u16 {
        let old = self.id();
        self.set_id(id);
        old
    }
}

macro_rules! unsupported_icmp_message_type {
    ($message:ty, $($ips:ty),+) => {
        impl MaybeTransportPacket for $message {
            fn transport_packet_data(&self) -> Option<TransportPacketData> {
                None
            }
        }

        $(
            impl IcmpMessage<$ips> for $message {
                const IS_REWRITABLE: bool = false;

                fn update_icmp_id(&mut self, _: u16) -> u16 {
                    unreachable!("non-echo ICMP packets should never be rewritten")
                }
            }
        )+
    };
}

unsupported_icmp_message_type!(Icmpv4TimestampRequest, Ipv4);
unsupported_icmp_message_type!(Icmpv4TimestampReply, Ipv4);
unsupported_icmp_message_type!(NeighborSolicitation, Ipv6);
unsupported_icmp_message_type!(NeighborAdvertisement, Ipv6);
unsupported_icmp_message_type!(RouterSolicitation, Ipv6);
unsupported_icmp_message_type!(MulticastListenerDone, Ipv6);
unsupported_icmp_message_type!(MulticastListenerReport, Ipv6);
unsupported_icmp_message_type!(MulticastListenerReportV2, Ipv6);
unsupported_icmp_message_type!(MulticastListenerQuery, Ipv6);
unsupported_icmp_message_type!(MulticastListenerQueryV2, Ipv6);
unsupported_icmp_message_type!(RouterAdvertisement, Ipv6);
// This isn't considered an error because, unlike ICMPv4, an ICMPv6 Redirect
// message doesn't contain an IP packet payload (RFC 2461 Section 4.5).
unsupported_icmp_message_type!(Redirect, Ipv6);

/// Implement For ICMP message that aren't errors.
macro_rules! non_error_icmp_message_type {
    ($message:ty, $ip:ty) => {
        impl<Inner> MaybeIcmpErrorPayload<$ip> for Nested<Inner, IcmpPacketBuilder<$ip, $message>> {
            fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<$ip>> {
                None
            }
        }

        impl<Inner> MaybeIcmpErrorMut<$ip> for Nested<Inner, IcmpPacketBuilder<$ip, $message>> {
            type IcmpErrorMut<'a>
                = Never
            where
                Self: 'a;

            fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
                None
            }
        }

        impl<Inner> DynamicMaybeIcmpErrorMut<$ip>
            for Nested<Inner, IcmpPacketBuilder<$ip, $message>>
        {
            fn dyn_icmp_error_mut(&mut self) -> Option<&mut dyn DynamicIcmpErrorMut<$ip>> {
                MaybeIcmpErrorMut::icmp_error_mut(self).map(|x| match x {})
            }
        }
    };
}

non_error_icmp_message_type!(Icmpv4TimestampRequest, Ipv4);
non_error_icmp_message_type!(Icmpv4TimestampReply, Ipv4);
non_error_icmp_message_type!(RouterSolicitation, Ipv6);
non_error_icmp_message_type!(RouterAdvertisement, Ipv6);
non_error_icmp_message_type!(NeighborSolicitation, Ipv6);
non_error_icmp_message_type!(NeighborAdvertisement, Ipv6);
non_error_icmp_message_type!(MulticastListenerReport, Ipv6);
non_error_icmp_message_type!(MulticastListenerDone, Ipv6);
non_error_icmp_message_type!(MulticastListenerReportV2, Ipv6);

macro_rules! icmp_error_message {
    ($message:ty, $($ips:ty),+) => {
        impl MaybeTransportPacket for $message {
            fn transport_packet_data(&self) -> Option<TransportPacketData> {
                None
            }
        }

        $(
            impl IcmpMessage<$ips> for $message {
                const IS_REWRITABLE: bool = false;

                fn update_icmp_id(&mut self, _: u16) -> u16 {
                    unreachable!("non-echo ICMP packets should never be rewritten")
                }
            }
        )+
    };
}

icmp_error_message!(IcmpDestUnreachable, Ipv4, Ipv6);
icmp_error_message!(IcmpTimeExceeded, Ipv4, Ipv6);
icmp_error_message!(Icmpv4ParameterProblem, Ipv4);
icmp_error_message!(Icmpv4Redirect, Ipv4);
icmp_error_message!(Icmpv6ParameterProblem, Ipv6);
icmp_error_message!(Icmpv6PacketTooBig, Ipv6);

macro_rules! icmpv4_error_message {
    ($message: ty) => {
        impl<Inner: AsRef<[u8]>> MaybeIcmpErrorPayload<Ipv4>
            for Nested<Inner, IcmpPacketBuilder<Ipv4, $message>>
        {
            fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<Ipv4>> {
                ParsedIcmpErrorPayload::parse_in_icmpv4_error(Buf::new(self.inner(), ..))
            }
        }

        impl<Inner: BufferMut> MaybeIcmpErrorMut<Ipv4>
            for Nested<Inner, IcmpPacketBuilder<Ipv4, $message>>
        {
            type IcmpErrorMut<'a>
                = &'a mut Self
            where
                Self: 'a;

            fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
                Some(self)
            }
        }

        impl<Inner: BufferMut> DynamicMaybeIcmpErrorMut<Ipv4>
            for Nested<Inner, IcmpPacketBuilder<Ipv4, $message>>
        {
            fn dyn_icmp_error_mut(&mut self) -> Option<&mut dyn DynamicIcmpErrorMut<Ipv4>> {
                MaybeIcmpErrorMut::icmp_error_mut(self).map(|x| x as _)
            }
        }

        impl<Inner: BufferMut> IcmpErrorMut<Ipv4>
            for Nested<Inner, IcmpPacketBuilder<Ipv4, $message>>
        {
            type InnerPacket<'a>
                = Ipv4PacketRaw<&'a mut [u8]>
            where
                Self: 'a;

            fn recalculate_checksum(&mut self) -> bool {
                // Checksum is calculated during serialization.
                true
            }

            fn inner_packet<'a>(&'a mut self) -> Option<Self::InnerPacket<'a>> {
                let packet =
                    Ipv4PacketRaw::parse_mut(SliceBufViewMut::new(self.inner_mut().as_mut()), ())
                        .ok()?;

                Some(packet)
            }
        }

        impl<Inner: BufferMut> DynamicIcmpErrorMut<Ipv4>
            for Nested<Inner, IcmpPacketBuilder<Ipv4, $message>>
        {
            fn dyn_recalculate_checksum(&mut self) -> bool {
                self.recalculate_checksum()
            }

            fn dyn_inner_packet(&mut self) -> Option<Ipv4PacketRaw<&mut [u8]>> {
                self.inner_packet()
            }
        }
    };
}

icmpv4_error_message!(IcmpDestUnreachable);
icmpv4_error_message!(Icmpv4Redirect);
icmpv4_error_message!(IcmpTimeExceeded);
icmpv4_error_message!(Icmpv4ParameterProblem);

macro_rules! icmpv6_error_message {
    ($message: ty) => {
        impl<Inner: Buffer> MaybeIcmpErrorPayload<Ipv6>
            for Nested<TruncatingSerializer<Inner>, IcmpPacketBuilder<Ipv6, $message>>
        {
            fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<Ipv6>> {
                ParsedIcmpErrorPayload::parse_in_icmpv6_error(Buf::new(self.inner().buffer(), ..))
            }
        }

        impl<Inner: BufferMut> MaybeIcmpErrorMut<Ipv6>
            for Nested<TruncatingSerializer<Inner>, IcmpPacketBuilder<Ipv6, $message>>
        {
            type IcmpErrorMut<'a>
                = &'a mut Self
            where
                Self: 'a;

            fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
                Some(self)
            }
        }

        impl<Inner: BufferMut> DynamicMaybeIcmpErrorMut<Ipv6>
            for Nested<TruncatingSerializer<Inner>, IcmpPacketBuilder<Ipv6, $message>>
        {
            fn dyn_icmp_error_mut(&mut self) -> Option<&mut dyn DynamicIcmpErrorMut<Ipv6>> {
                MaybeIcmpErrorMut::icmp_error_mut(self).map(|x| x as _)
            }
        }

        impl<Inner: BufferMut> IcmpErrorMut<Ipv6>
            for Nested<TruncatingSerializer<Inner>, IcmpPacketBuilder<Ipv6, $message>>
        {
            type InnerPacket<'a>
                = Ipv6PacketRaw<&'a mut [u8]>
            where
                Self: 'a;

            fn recalculate_checksum(&mut self) -> bool {
                // Checksum is calculated during serialization.
                true
            }

            fn inner_packet<'a>(&'a mut self) -> Option<Self::InnerPacket<'a>> {
                let packet = Ipv6PacketRaw::parse_mut(
                    SliceBufViewMut::new(self.inner_mut().buffer_mut().as_mut()),
                    (),
                )
                .ok()?;

                Some(packet)
            }
        }

        impl<Inner: BufferMut> DynamicIcmpErrorMut<Ipv6>
            for Nested<TruncatingSerializer<Inner>, IcmpPacketBuilder<Ipv6, $message>>
        {
            fn dyn_recalculate_checksum(&mut self) -> bool {
                self.recalculate_checksum()
            }

            fn dyn_inner_packet(&mut self) -> Option<Ipv6PacketRaw<&mut [u8]>> {
                self.inner_packet()
            }
        }
    };
}

icmpv6_error_message!(IcmpDestUnreachable);
icmpv6_error_message!(Icmpv6PacketTooBig);
icmpv6_error_message!(IcmpTimeExceeded);
icmpv6_error_message!(Icmpv6ParameterProblem);

impl<M: igmp::MessageType<EmptyBuf>> MaybeIcmpErrorMut<Ipv4>
    for InnerSerializer<IgmpPacketBuilder<EmptyBuf, M>, EmptyBuf>
{
    type IcmpErrorMut<'a>
        = Never
    where
        Self: 'a;

    fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
        None
    }
}

impl<M: igmp::MessageType<EmptyBuf>> DynamicMaybeIcmpErrorMut<Ipv4>
    for InnerSerializer<IgmpPacketBuilder<EmptyBuf, M>, EmptyBuf>
{
    fn dyn_icmp_error_mut(&mut self) -> Option<&mut dyn DynamicIcmpErrorMut<Ipv4>> {
        self.icmp_error_mut().map(|x| match x {})
    }
}

impl<M: igmp::MessageType<EmptyBuf>> MaybeTransportPacket
    for InnerSerializer<IgmpPacketBuilder<EmptyBuf, M>, EmptyBuf>
{
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        None
    }
}

impl<M: igmp::MessageType<EmptyBuf>> DynamicMaybeTransportPacketMut<Ipv4>
    for InnerSerializer<IgmpPacketBuilder<EmptyBuf, M>, EmptyBuf>
{
    fn dyn_transport_packet_mut(&mut self) -> Option<&mut dyn TransportPacketMut<Ipv4>> {
        self.transport_packet_mut().map(|x| match x {})
    }
}

impl<M: igmp::MessageType<EmptyBuf>> MaybeTransportPacketMut<Ipv4>
    for InnerSerializer<IgmpPacketBuilder<EmptyBuf, M>, EmptyBuf>
{
    type TransportPacketMut<'a>
        = Never
    where
        M: 'a;

    fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
        None
    }
}

impl<I: IpExt, M: igmp::MessageType<EmptyBuf>> MaybeIcmpErrorPayload<I>
    for InnerSerializer<IgmpPacketBuilder<EmptyBuf, M>, EmptyBuf>
{
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
        None
    }
}

impl<I> MaybeTransportPacket for InnerSerializer<IgmpMembershipReportV3Builder<I>, EmptyBuf> {
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        None
    }
}

impl<I> MaybeTransportPacketMut<Ipv4>
    for InnerSerializer<IgmpMembershipReportV3Builder<I>, EmptyBuf>
{
    type TransportPacketMut<'a>
        = Never
    where
        I: 'a;

    fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
        None
    }
}

impl<I> DynamicMaybeTransportPacketMut<Ipv4>
    for InnerSerializer<IgmpMembershipReportV3Builder<I>, EmptyBuf>
{
    fn dyn_transport_packet_mut(&mut self) -> Option<&mut dyn TransportPacketMut<Ipv4>> {
        self.transport_packet_mut().map(|x| match x {})
    }
}

impl<I: IpExt, II, B> MaybeIcmpErrorPayload<I>
    for InnerSerializer<IgmpMembershipReportV3Builder<II>, B>
{
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
        None
    }
}

impl<I, B> MaybeIcmpErrorMut<Ipv4> for InnerSerializer<IgmpMembershipReportV3Builder<I>, B> {
    type IcmpErrorMut<'a>
        = Never
    where
        Self: 'a;

    fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
        None
    }
}

impl<I, B> DynamicMaybeIcmpErrorMut<Ipv4> for InnerSerializer<IgmpMembershipReportV3Builder<I>, B> {
    fn dyn_icmp_error_mut(&mut self) -> Option<&mut dyn DynamicIcmpErrorMut<Ipv4>> {
        self.icmp_error_mut().map(|x| match x {})
    }
}

impl<I> MaybeTransportPacket
    for EitherSerializer<
        EmptyBuf,
        InnerSerializer<packet::records::RecordSequenceBuilder<NdpOptionBuilder<'_>, I>, EmptyBuf>,
    >
{
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        None
    }
}

/// An unsanitized IP packet body.
///
/// Allows packets from raw IP sockets (with a user provided IP body), to be
/// tracked from the filtering module.
#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct RawIpBody<I: IpExt, B: ParseBuffer> {
    /// The IANA protocol of the inner message. This may be, but is not required
    /// to be, a transport protocol.
    protocol: I::Proto,
    /// The source IP addr of the packet. Required by
    /// [`ParsedTransportHeaderMut`] to recompute checksums.
    src_addr: I::Addr,
    /// The destination IP addr of the packet. Required by
    /// [`ParsedTransportHeaderMut`] to recompute checksums.
    dst_addr: I::Addr,
    /// The body of the IP packet. The body is expected to be a message of type
    /// `protocol`, but is not guaranteed to be valid.
    body: B,
    /// The parsed transport data contained within `body`. Only `Some` if body
    /// is a valid transport header.
    transport_packet_data: Option<TransportPacketData>,
}

impl<I: IpExt, B: ParseBuffer> RawIpBody<I, B> {
    /// Construct a new [`RawIpBody`] from it's parts.
    pub fn new(
        protocol: I::Proto,
        src_addr: I::Addr,
        dst_addr: I::Addr,
        body: B,
    ) -> RawIpBody<I, B> {
        let transport_packet_data = TransportPacketData::parse_in_ip_packet::<I, _>(
            src_addr,
            dst_addr,
            protocol,
            Buf::new(&body, ..),
        );
        RawIpBody { protocol, src_addr, dst_addr, body, transport_packet_data }
    }
}

impl<I: IpExt, B: ParseBuffer> MaybeTransportPacket for RawIpBody<I, B> {
    fn transport_packet_data(&self) -> Option<TransportPacketData> {
        self.transport_packet_data.clone()
    }
}

impl<I: IpExt, B: BufferMut> MaybeTransportPacketMut<I> for RawIpBody<I, B> {
    type TransportPacketMut<'a>
        = ParsedTransportHeaderMut<'a, I>
    where
        Self: 'a;

    fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
        let RawIpBody { protocol, src_addr: _, dst_addr: _, body, transport_packet_data: _ } = self;
        ParsedTransportHeaderMut::<I>::parse_in_ip_packet(
            *protocol,
            SliceBufViewMut::new(body.as_mut()),
        )
    }
}

impl<I: IpExt, B: ParseBuffer> MaybeIcmpErrorPayload<I> for RawIpBody<I, B> {
    fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
        ParsedIcmpErrorPayload::parse_in_outer_ip_packet(self.protocol, Buf::new(&self.body, ..))
    }
}

impl<I: FilterIpExt, B: BufferMut> MaybeIcmpErrorMut<I> for RawIpBody<I, B> {
    type IcmpErrorMut<'a>
        = ParsedIcmpErrorMut<'a, I>
    where
        Self: 'a;

    fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
        let RawIpBody { protocol, src_addr, dst_addr, body, transport_packet_data: _ } = self;

        ParsedIcmpErrorMut::parse_in_ip_packet(
            *src_addr,
            *dst_addr,
            *protocol,
            SliceBufViewMut::new(body.as_mut()),
        )
    }
}

impl<I: IpExt, B: BufferMut + NetworkSerializer> Serializer<NetworkSerializationContext>
    for RawIpBody<I, B>
{
    type Buffer = <B as Serializer<NetworkSerializationContext>>::Buffer;

    fn serialize<G: GrowBufferMut, P: BufferProvider<Self::Buffer, G>>(
        self,
        context: &mut NetworkSerializationContext,
        constraints: PacketConstraints,
        provider: P,
    ) -> Result<G, (SerializeError<P::Error>, Self)> {
        let Self { protocol, src_addr, dst_addr, body, transport_packet_data } = self;
        body.serialize(context, constraints, provider).map_err(|(err, body)| {
            (err, Self { protocol, src_addr, dst_addr, body, transport_packet_data })
        })
    }

    fn serialize_new_buf<BB: GrowBufferMut, A: LayoutBufferAlloc<BB>>(
        &self,
        context: &mut NetworkSerializationContext,
        outer: PacketConstraints,
        alloc: A,
    ) -> Result<BB, SerializeError<A::Error>> {
        self.body.serialize_new_buf(context, outer, alloc)
    }
}

impl<I: IpExt, B: BufferMut + NetworkSerializer> NestableSerializer for RawIpBody<I, B> {}

impl<I: IpExt, B: BufferMut> PartialSerializer<NetworkSerializationContext> for RawIpBody<I, B> {
    fn partial_serialize<BB: GrowBufferMut + ContiguousBuffer, A: LayoutBufferAlloc<BB>>(
        &self,
        _context: &mut NetworkSerializationContext,
        _alloc: A,
    ) -> Result<PartialSerializeResult<'_, BB>, SerializeError<A::Error>> {
        Ok(PartialSerializeResult::Slice(self.body.as_ref()))
    }

    fn partial_serialize_new_buf<BB: GrowBufferMut, A: LayoutBufferAlloc<BB>>(
        &self,
        _context: &mut NetworkSerializationContext,
        constraints: PacketConstraints,
        alloc: A,
    ) -> Result<(BB, usize), SerializeError<A::Error>> {
        let bytes_to_copy = cmp::min(self.body.len(), TRANSPORT_HEADER_MAX_SIZE);
        let header_len = constraints.header_len();
        let mut buffer = alloc.layout_alloc(header_len, bytes_to_copy, 0)?;
        buffer.with_parts_mut(|_prefix, mut body, _suffix| {
            body.copy_from_slice(&self.body.as_ref()[..bytes_to_copy]);
        });
        let total_size = cmp::max(
            constraints.min_body_len(),
            cmp::min(self.body.len(), constraints.max_body_len()),
        );
        Ok((buffer, total_size))
    }
}

fn parse_transport_header_in_ipv4_packet<B: ParseBuffer>(
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    proto: Ipv4Proto,
    body: B,
) -> Option<TransportPacketData> {
    match proto {
        Ipv4Proto::Proto(IpProto::Udp) => parse_udp_header::<_, Ipv4>(body),
        Ipv4Proto::Proto(IpProto::Tcp) => parse_tcp_header::<_, Ipv4>(body, src_ip, dst_ip),
        Ipv4Proto::Icmp => parse_icmpv4_header(body),
        Ipv4Proto::Proto(IpProto::Reserved) | Ipv4Proto::Igmp | Ipv4Proto::Other(_) => None,
    }
}

fn parse_transport_header_in_ipv6_packet<B: ParseBuffer>(
    src_ip: Ipv6Addr,
    dst_ip: Ipv6Addr,
    proto: Ipv6Proto,
    body: B,
) -> Option<TransportPacketData> {
    match proto {
        Ipv6Proto::Proto(IpProto::Udp) => parse_udp_header::<_, Ipv6>(body),
        Ipv6Proto::Proto(IpProto::Tcp) => parse_tcp_header::<_, Ipv6>(body, src_ip, dst_ip),
        Ipv6Proto::Icmpv6 => parse_icmpv6_header(body),
        Ipv6Proto::Proto(IpProto::Reserved) | Ipv6Proto::NoNextHeader | Ipv6Proto::Other(_) => None,
    }
}

fn parse_udp_header<B: ParseBuffer, I: Ip>(mut body: B) -> Option<TransportPacketData> {
    let packet = body.parse_with::<_, UdpPacketRaw<_>>(I::VERSION_MARKER).ok()?;
    Some(TransportPacketData::Generic {
        src_port: packet.src_port().map(NonZeroU16::get).unwrap_or(0),
        // NB: UDP packets must have a specified (nonzero) destination port, so
        // if this packet has a destination port of 0, it is malformed.
        dst_port: packet.dst_port()?.get(),
    })
}

fn parse_tcp_header<B: ParseBuffer, I: IpExt>(
    mut body: B,
    src_ip: I::Addr,
    dst_ip: I::Addr,
) -> Option<TransportPacketData> {
    // NOTE: By using TcpSegmentRaw here, we're opting into getting invalid data
    // (for example, if the checksum isn't valid). As a team, we've decided
    // that's okay for now, since the worst that happens is we filter or
    // conntrack a packet incorrectly and the end host rejects it.
    //
    // This will be fixed at some point as part of a larger effort to ensure
    // that checksums are validated exactly once (and hopefully via checksum
    // offloading).
    let packet = body.parse::<TcpSegmentRaw<_>>().ok()?;

    let (fallback_src_port, fallback_dst_port) = packet.flow_header().src_dst();
    let fallback =
        TransportPacketData::Generic { src_port: fallback_src_port, dst_port: fallback_dst_port };

    // TODO(https://fxbug.dev/328064909): When we enable configurable dropping of
    // invalid packets, we're going to want to bubble up the detection of a
    // truncated packet or invalid flags into the hooks in logic.rs (maybe coming
    // out of `IpPacket::conntrack_packet()`).
    let (builder, options_res, body) = match packet.into_builder_options(src_ip, dst_ip) {
        Ok(x) => x,
        Err(_) => return Some(fallback),
    };
    let options = match options_res {
        Ok(options) => options,
        Err((options, _err)) => options,
    };
    let options = match Options::try_from_options(&builder, &options) {
        Ok(x) => x,
        Err(MalformedFlags { .. }) => return Some(fallback),
    };

    let segment = match SegmentHeader::from_builder_options(&builder, options) {
        Ok(x) => x,
        Err(MalformedFlags { .. }) => return Some(fallback),
    };

    Some(TransportPacketData::Tcp {
        src_port: builder.src_port().map(NonZeroU16::get).unwrap_or(0),
        dst_port: builder.dst_port().map(NonZeroU16::get).unwrap_or(0),
        segment,
        payload_len: body.len(),
    })
}

fn parse_icmpv4_header<B: ParseBuffer>(mut body: B) -> Option<TransportPacketData> {
    match icmp::peek_message_type(body.as_ref()).ok()? {
        Icmpv4MessageType::EchoRequest => {
            let packet = body.parse::<IcmpPacketRaw<Ipv4, _, IcmpEchoRequest>>().ok()?;
            packet.message().transport_packet_data()
        }
        Icmpv4MessageType::EchoReply => {
            let packet = body.parse::<IcmpPacketRaw<Ipv4, _, IcmpEchoReply>>().ok()?;
            packet.message().transport_packet_data()
        }
        // ICMP errors have a separate parsing path.
        Icmpv4MessageType::DestUnreachable
        | Icmpv4MessageType::Redirect
        | Icmpv4MessageType::TimeExceeded
        | Icmpv4MessageType::ParameterProblem => None,
        // NOTE: If these are parsed, then without further work, conntrack won't
        // be able to differentiate between these and ECHO message with the same
        // ID.
        Icmpv4MessageType::TimestampRequest | Icmpv4MessageType::TimestampReply => None,
    }
}

fn parse_icmpv6_header<B: ParseBuffer>(mut body: B) -> Option<TransportPacketData> {
    match icmp::peek_message_type(body.as_ref()).ok()? {
        Icmpv6MessageType::EchoRequest => {
            let packet = body.parse::<IcmpPacketRaw<Ipv6, _, IcmpEchoRequest>>().ok()?;
            packet.message().transport_packet_data()
        }
        Icmpv6MessageType::EchoReply => {
            let packet = body.parse::<IcmpPacketRaw<Ipv6, _, IcmpEchoReply>>().ok()?;
            packet.message().transport_packet_data()
        }
        // ICMP errors have a separate parsing path.
        Icmpv6MessageType::DestUnreachable
        | Icmpv6MessageType::PacketTooBig
        | Icmpv6MessageType::TimeExceeded
        | Icmpv6MessageType::ParameterProblem => None,
        Icmpv6MessageType::RouterSolicitation
        | Icmpv6MessageType::RouterAdvertisement
        | Icmpv6MessageType::NeighborSolicitation
        | Icmpv6MessageType::NeighborAdvertisement
        | Icmpv6MessageType::Redirect
        | Icmpv6MessageType::MulticastListenerQuery
        | Icmpv6MessageType::MulticastListenerReport
        | Icmpv6MessageType::MulticastListenerDone
        | Icmpv6MessageType::MulticastListenerReportV2 => None,
    }
}

/// A transport header that has been parsed from a byte buffer and provides
/// mutable access to its contents.
#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub enum ParsedTransportHeaderMut<'a, I: IpExt> {
    Tcp(TcpSegmentRaw<&'a mut [u8]>),
    Udp(UdpPacketRaw<&'a mut [u8]>),
    Icmp(I::IcmpPacketTypeRaw<&'a mut [u8]>),
}

impl<'a> ParsedTransportHeaderMut<'a, Ipv4> {
    fn parse_in_ipv4_packet<BV: BufferViewMut<&'a mut [u8]>>(
        proto: Ipv4Proto,
        body: BV,
    ) -> Option<Self> {
        match proto {
            Ipv4Proto::Proto(IpProto::Udp) => {
                Some(Self::Udp(UdpPacketRaw::parse_mut(body, IpVersionMarker::<Ipv4>::new()).ok()?))
            }
            Ipv4Proto::Proto(IpProto::Tcp) => {
                Some(Self::Tcp(TcpSegmentRaw::parse_mut(body, ()).ok()?))
            }
            Ipv4Proto::Icmp => Some(Self::Icmp(Icmpv4PacketRaw::parse_mut(body, ()).ok()?)),
            Ipv4Proto::Proto(IpProto::Reserved) | Ipv4Proto::Igmp | Ipv4Proto::Other(_) => None,
        }
    }
}

impl<'a> ParsedTransportHeaderMut<'a, Ipv6> {
    fn parse_in_ipv6_packet<BV: BufferViewMut<&'a mut [u8]>>(
        proto: Ipv6Proto,
        body: BV,
    ) -> Option<Self> {
        match proto {
            Ipv6Proto::Proto(IpProto::Udp) => {
                Some(Self::Udp(UdpPacketRaw::parse_mut(body, IpVersionMarker::<Ipv6>::new()).ok()?))
            }
            Ipv6Proto::Proto(IpProto::Tcp) => {
                Some(Self::Tcp(TcpSegmentRaw::parse_mut(body, ()).ok()?))
            }
            Ipv6Proto::Icmpv6 => Some(Self::Icmp(Icmpv6PacketRaw::parse_mut(body, ()).ok()?)),
            Ipv6Proto::Proto(IpProto::Reserved) | Ipv6Proto::NoNextHeader | Ipv6Proto::Other(_) => {
                None
            }
        }
    }
}

impl<'a, I: IpExt> ParsedTransportHeaderMut<'a, I> {
    fn parse_in_ip_packet<BV: BufferViewMut<&'a mut [u8]>>(
        proto: I::Proto,
        body: BV,
    ) -> Option<Self> {
        I::map_ip(
            (proto, IpInvariant(body)),
            |(proto, IpInvariant(body))| {
                ParsedTransportHeaderMut::<'a, Ipv4>::parse_in_ipv4_packet(proto, body)
            },
            |(proto, IpInvariant(body))| {
                ParsedTransportHeaderMut::<'a, Ipv6>::parse_in_ipv6_packet(proto, body)
            },
        )
    }

    fn update_pseudo_header_address(&mut self, old: I::Addr, new: I::Addr) {
        match self {
            Self::Tcp(segment) => segment.update_checksum_pseudo_header_address(old, new),
            Self::Udp(packet) => {
                packet.update_checksum_pseudo_header_address(old, new);
            }
            Self::Icmp(packet) => {
                packet.update_checksum_pseudo_header_address(old, new);
            }
        }
    }
}

/// An inner IP packet contained within an ICMP error.
#[derive(Debug, PartialEq, Eq, GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct ParsedIcmpErrorPayload<I: IpExt> {
    src_ip: I::Addr,
    dst_ip: I::Addr,
    // Hold the ports directly instead of TransportPacketData. In case of an
    // ICMP error, we don't update conntrack connection state, so there's no
    // reason to keep the extra information.
    src_port: u16,
    dst_port: u16,
    proto: I::Proto,
}

impl ParsedIcmpErrorPayload<Ipv4> {
    fn parse_in_outer_ipv4_packet<B>(protocol: Ipv4Proto, mut body: B) -> Option<Self>
    where
        B: ParseBuffer,
    {
        match protocol {
            Ipv4Proto::Proto(_) | Ipv4Proto::Igmp | Ipv4Proto::Other(_) => None,
            Ipv4Proto::Icmp => {
                let message = body.parse::<Icmpv4PacketRaw<_>>().ok()?;
                let message_body = match &message {
                    Icmpv4PacketRaw::EchoRequest(_)
                    | Icmpv4PacketRaw::EchoReply(_)
                    | Icmpv4PacketRaw::TimestampRequest(_)
                    | Icmpv4PacketRaw::TimestampReply(_) => return None,

                    Icmpv4PacketRaw::DestUnreachable(inner) => inner.message_body(),
                    Icmpv4PacketRaw::Redirect(inner) => inner.message_body(),
                    Icmpv4PacketRaw::TimeExceeded(inner) => inner.message_body(),
                    Icmpv4PacketRaw::ParameterProblem(inner) => inner.message_body(),
                };

                Self::parse_in_icmpv4_error(Buf::new(message_body, ..))
            }
        }
    }

    fn parse_in_icmpv4_error<B>(mut body: B) -> Option<Self>
    where
        B: ParseBuffer,
    {
        let packet = body.parse::<Ipv4PacketRaw<_>>().ok()?;

        let src_ip = packet.get_header_prefix().src_ip();
        let dst_ip = packet.get_header_prefix().dst_ip();
        let proto = packet.proto();
        let transport_data = parse_transport_header_in_ipv4_packet(
            src_ip,
            dst_ip,
            proto,
            packet.body().into_inner(),
        )?;
        Some(Self {
            src_ip,
            dst_ip,
            src_port: transport_data.src_port(),
            dst_port: transport_data.dst_port(),
            proto,
        })
    }
}

impl ParsedIcmpErrorPayload<Ipv6> {
    fn parse_in_outer_ipv6_packet<B>(protocol: Ipv6Proto, mut body: B) -> Option<Self>
    where
        B: ParseBuffer,
    {
        match protocol {
            Ipv6Proto::NoNextHeader | Ipv6Proto::Proto(_) | Ipv6Proto::Other(_) => None,

            Ipv6Proto::Icmpv6 => {
                let message = body.parse::<Icmpv6PacketRaw<_>>().ok()?;
                let message_body = match &message {
                    Icmpv6PacketRaw::EchoRequest(_)
                    | Icmpv6PacketRaw::EchoReply(_)
                    | Icmpv6PacketRaw::Ndp(_)
                    | Icmpv6PacketRaw::Mld(_) => return None,

                    Icmpv6PacketRaw::DestUnreachable(inner) => inner.message_body(),
                    Icmpv6PacketRaw::PacketTooBig(inner) => inner.message_body(),
                    Icmpv6PacketRaw::TimeExceeded(inner) => inner.message_body(),
                    Icmpv6PacketRaw::ParameterProblem(inner) => inner.message_body(),
                };

                Self::parse_in_icmpv6_error(Buf::new(message_body, ..))
            }
        }
    }

    fn parse_in_icmpv6_error<B>(mut body: B) -> Option<Self>
    where
        B: ParseBuffer,
    {
        let packet = body.parse::<Ipv6PacketRaw<_>>().ok()?;

        let src_ip = packet.get_fixed_header().src_ip();
        let dst_ip = packet.get_fixed_header().dst_ip();
        let proto = packet.proto().ok()?;
        let transport_data = parse_transport_header_in_ipv6_packet(
            src_ip,
            dst_ip,
            proto,
            packet.body().ok()?.into_inner(),
        )?;
        Some(Self {
            src_ip,
            dst_ip,
            src_port: transport_data.src_port(),
            dst_port: transport_data.dst_port(),
            proto,
        })
    }
}

impl<I: IpExt> ParsedIcmpErrorPayload<I> {
    fn parse_in_outer_ip_packet<B>(proto: I::Proto, body: B) -> Option<Self>
    where
        B: ParseBuffer,
    {
        I::map_ip(
            (proto, IpInvariant(body)),
            |(proto, IpInvariant(body))| {
                ParsedIcmpErrorPayload::<Ipv4>::parse_in_outer_ipv4_packet(proto, body)
            },
            |(proto, IpInvariant(body))| {
                ParsedIcmpErrorPayload::<Ipv6>::parse_in_outer_ipv6_packet(proto, body)
            },
        )
    }
}

/// An ICMP error packet that provides mutable access to the contained IP
/// packet.
#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct ParsedIcmpErrorMut<'a, I: IpExt> {
    src_ip: I::Addr,
    dst_ip: I::Addr,
    message: I::IcmpPacketTypeRaw<&'a mut [u8]>,
}

impl<'a> ParsedIcmpErrorMut<'a, Ipv4> {
    fn parse_in_ipv4_packet<BV: BufferViewMut<&'a mut [u8]>>(
        src_ip: Ipv4Addr,
        dst_ip: Ipv4Addr,
        proto: Ipv4Proto,
        body: BV,
    ) -> Option<Self> {
        match proto {
            Ipv4Proto::Proto(_) | Ipv4Proto::Igmp | Ipv4Proto::Other(_) => None,
            Ipv4Proto::Icmp => {
                let message = Icmpv4PacketRaw::parse_mut(body, ()).ok()?;
                match message {
                    Icmpv4PacketRaw::EchoRequest(_)
                    | Icmpv4PacketRaw::EchoReply(_)
                    | Icmpv4PacketRaw::TimestampRequest(_)
                    | Icmpv4PacketRaw::TimestampReply(_) => None,

                    Icmpv4PacketRaw::DestUnreachable(_)
                    | Icmpv4PacketRaw::Redirect(_)
                    | Icmpv4PacketRaw::TimeExceeded(_)
                    | Icmpv4PacketRaw::ParameterProblem(_) => {
                        Some(Self { src_ip, dst_ip, message })
                    }
                }
            }
        }
    }
}

impl<'a> ParsedIcmpErrorMut<'a, Ipv6> {
    fn parse_in_ipv6_packet<BV: BufferViewMut<&'a mut [u8]>>(
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        proto: Ipv6Proto,
        body: BV,
    ) -> Option<Self> {
        match proto {
            Ipv6Proto::NoNextHeader | Ipv6Proto::Proto(_) | Ipv6Proto::Other(_) => None,

            Ipv6Proto::Icmpv6 => {
                let message = Icmpv6PacketRaw::parse_mut(body, ()).ok()?;
                match message {
                    Icmpv6PacketRaw::EchoRequest(_)
                    | Icmpv6PacketRaw::EchoReply(_)
                    | Icmpv6PacketRaw::Ndp(_)
                    | Icmpv6PacketRaw::Mld(_) => None,

                    Icmpv6PacketRaw::DestUnreachable(_)
                    | Icmpv6PacketRaw::PacketTooBig(_)
                    | Icmpv6PacketRaw::TimeExceeded(_)
                    | Icmpv6PacketRaw::ParameterProblem(_) => {
                        Some(Self { src_ip, dst_ip, message })
                    }
                }
            }
        }
    }
}

impl<'a, I: FilterIpExt> ParsedIcmpErrorMut<'a, I> {
    fn parse_in_ip_packet<BV: BufferViewMut<&'a mut [u8]>>(
        src_ip: I::Addr,
        dst_ip: I::Addr,
        proto: I::Proto,
        body: BV,
    ) -> Option<Self> {
        I::map_ip(
            (src_ip, dst_ip, proto, IpInvariant(body)),
            |(src_ip, dst_ip, proto, IpInvariant(body))| {
                ParsedIcmpErrorMut::<'a, Ipv4>::parse_in_ipv4_packet(src_ip, dst_ip, proto, body)
            },
            |(src_ip, dst_ip, proto, IpInvariant(body))| {
                ParsedIcmpErrorMut::<'a, Ipv6>::parse_in_ipv6_packet(src_ip, dst_ip, proto, body)
            },
        )
    }
}

impl<'a, I: FilterIpExt> IcmpErrorMut<I> for ParsedIcmpErrorMut<'a, I> {
    type InnerPacket<'b>
        = I::FilterIpPacketRaw<&'b mut [u8]>
    where
        Self: 'b;

    fn inner_packet<'b>(&'b mut self) -> Option<Self::InnerPacket<'b>> {
        Some(I::as_filter_packet_raw_owned(
            I::PacketRaw::parse_mut(SliceBufViewMut::new(self.message.message_body_mut()), ())
                .ok()?,
        ))
    }

    fn recalculate_checksum(&mut self) -> bool {
        let Self { src_ip, dst_ip, message } = self;
        message.try_write_checksum(*src_ip, *dst_ip)
    }
}

/// A helper trait to extract [`IcmpMessage`] impls from parsed ICMP messages.
trait IcmpMessageImplHelper<I: IpExt> {
    fn message_impl_mut(&mut self) -> &mut impl IcmpMessage<I>;
}

impl<I: IpExt, B: SplitByteSliceMut, M: IcmpMessage<I>> IcmpMessageImplHelper<I>
    for IcmpPacketRaw<I, B, M>
{
    fn message_impl_mut(&mut self) -> &mut impl IcmpMessage<I> {
        self.message_mut()
    }
}

impl<'a, I: IpExt> TransportPacketMut<I> for ParsedTransportHeaderMut<'a, I> {
    fn set_src_port(&mut self, port: NonZeroU16) {
        match self {
            ParsedTransportHeaderMut::Tcp(segment) => segment.set_src_port(port),
            ParsedTransportHeaderMut::Udp(packet) => packet.set_src_port(port.get()),
            ParsedTransportHeaderMut::Icmp(packet) => {
                I::map_ip::<_, ()>(
                    packet,
                    |packet| {
                        packet_formats::icmpv4_dispatch!(
                            packet: raw,
                            p => {
                                let message = p.message_impl_mut();
                                if  message.is_rewritable() {
                                    let old = message.update_icmp_id(port.get());
                                    p.update_checksum_header_field_u16(old, port.get())
                                }
                            }
                        );
                    },
                    |packet| {
                        packet_formats::icmpv6_dispatch!(
                            packet: raw,
                            p => {
                                let message = p.message_impl_mut();
                                if  message.is_rewritable() {
                                    let old = message.update_icmp_id(port.get());
                                    p.update_checksum_header_field_u16(old, port.get())
                                }
                            }
                        );
                    },
                );
            }
        }
    }

    fn set_dst_port(&mut self, port: NonZeroU16) {
        match self {
            ParsedTransportHeaderMut::Tcp(segment) => segment.set_dst_port(port),
            ParsedTransportHeaderMut::Udp(packet) => packet.set_dst_port(port),
            ParsedTransportHeaderMut::Icmp(packet) => {
                I::map_ip::<_, ()>(
                    packet,
                    |packet| {
                        packet_formats::icmpv4_dispatch!(
                            packet:raw,
                            p => {
                                let message = p.message_impl_mut();
                                if  message.is_rewritable() {
                                    let old = message.update_icmp_id(port.get());
                                    p.update_checksum_header_field_u16(old, port.get())
                                }
                            }
                        );
                    },
                    |packet| {
                        packet_formats::icmpv6_dispatch!(
                            packet:raw,
                            p => {
                                let message = p.message_impl_mut();
                                if  message.is_rewritable() {
                                    let old = message.update_icmp_id(port.get());
                                    p.update_checksum_header_field_u16(old, port.get())
                                }
                            }
                        );
                    },
                );
            }
        }
    }

    fn update_pseudo_header_src_addr(&mut self, old: I::Addr, new: I::Addr) {
        self.update_pseudo_header_address(old, new);
    }

    fn update_pseudo_header_dst_addr(&mut self, old: I::Addr, new: I::Addr) {
        self.update_pseudo_header_address(old, new);
    }
}

#[cfg(any(test, feature = "testutils"))]
pub mod testutil {
    use super::*;

    // Note that we could choose to implement `MaybeTransportPacket` for these
    // opaque byte buffer types by parsing them as we do incoming buffers, but since
    // these implementations are only for use in netstack3_core unit tests, there is
    // no expectation that filtering or connection tracking actually be performed.
    // If that changes at some point, we could replace these with "real"
    // implementations.

    impl<B: BufferMut> MaybeTransportPacket for Nested<B, ()> {
        fn transport_packet_data(&self) -> Option<TransportPacketData> {
            unimplemented!()
        }
    }

    impl<I: IpExt, B: BufferMut> MaybeTransportPacketMut<I> for Nested<B, ()> {
        type TransportPacketMut<'a>
            = Never
        where
            B: 'a;

        fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
            unimplemented!()
        }
    }

    impl<I: IpExt, B: BufferMut> MaybeIcmpErrorPayload<I> for Nested<B, ()> {
        fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
            unimplemented!()
        }
    }

    impl<I: FilterIpExt, B: BufferMut> MaybeIcmpErrorMut<I> for Nested<B, ()> {
        type IcmpErrorMut<'a>
            = Never
        where
            Self: 'a;

        fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
            unimplemented!()
        }
    }

    impl MaybeTransportPacket for InnerSerializer<&[u8], EmptyBuf> {
        fn transport_packet_data(&self) -> Option<TransportPacketData> {
            None
        }
    }

    impl<I: IpExt> MaybeTransportPacketMut<I> for InnerSerializer<&[u8], EmptyBuf> {
        type TransportPacketMut<'a>
            = Never
        where
            Self: 'a;

        fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
            None
        }
    }

    impl<I: IpExt> MaybeIcmpErrorPayload<I> for InnerSerializer<&[u8], EmptyBuf> {
        fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
            None
        }
    }

    impl<I: FilterIpExt> MaybeIcmpErrorMut<I> for InnerSerializer<&[u8], EmptyBuf> {
        type IcmpErrorMut<'a>
            = Never
        where
            Self: 'a;

        fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
            None
        }
    }

    #[cfg(test)]
    pub(crate) mod internal {
        use alloc::vec::Vec;
        use net_declare::{net_ip_v4, net_ip_v6, net_subnet_v4, net_subnet_v6};
        use net_types::ip::Subnet;
        use netstack3_base::{SeqNum, UnscaledWindowSize};
        use packet::{PartialPacketBuilder as _, TruncateDirection};
        use packet_formats::icmp::{Icmpv4DestUnreachableCode, Icmpv6DestUnreachableCode};

        use super::*;

        pub trait TestIpExt: FilterIpExt {
            const SRC_IP: Self::Addr;
            const SRC_PORT: u16 = 1234;
            const DST_IP: Self::Addr;
            const DST_PORT: u16 = 9876;
            const SRC_IP_2: Self::Addr;
            const DST_IP_2: Self::Addr;
            const DST_IP_3: Self::Addr;
            const IP_OUTSIDE_SUBNET: Self::Addr;
            const SUBNET: Subnet<Self::Addr>;
            const PACKET_TTL: u8 = u8::MAX;
        }

        impl TestIpExt for Ipv4 {
            const SRC_IP: Self::Addr = net_ip_v4!("192.0.2.1");
            const DST_IP: Self::Addr = net_ip_v4!("192.0.2.2");
            const SRC_IP_2: Self::Addr = net_ip_v4!("192.0.2.3");
            const DST_IP_2: Self::Addr = net_ip_v4!("192.0.2.4");
            const DST_IP_3: Self::Addr = net_ip_v4!("192.0.2.6");
            const IP_OUTSIDE_SUBNET: Self::Addr = net_ip_v4!("192.0.3.1");
            const SUBNET: Subnet<Self::Addr> = net_subnet_v4!("192.0.2.0/24");
        }

        impl TestIpExt for Ipv6 {
            const SRC_IP: Self::Addr = net_ip_v6!("2001:db8::1");
            const DST_IP: Self::Addr = net_ip_v6!("2001:db8::2");
            const SRC_IP_2: Self::Addr = net_ip_v6!("2001:db8::3");
            const DST_IP_2: Self::Addr = net_ip_v6!("2001:db8::4");
            const DST_IP_3: Self::Addr = net_ip_v6!("2001:db8::6");
            const IP_OUTSIDE_SUBNET: Self::Addr = net_ip_v6!("2001:db8:ffff::1");
            const SUBNET: Subnet<Self::Addr> = net_subnet_v6!("2001:db8::/64");
        }

        #[derive(Clone, Debug, PartialEq)]
        pub struct FakeIpPacket<I: FilterIpExt, T>
        where
            for<'a> &'a T: TransportPacketExt<I>,
        {
            pub src_ip: I::Addr,
            pub dst_ip: I::Addr,
            pub body: T,
        }

        impl<I: FilterIpExt> FakeIpPacket<I, FakeUdpPacket> {
            pub(crate) fn reply(&self) -> Self {
                Self { src_ip: self.dst_ip, dst_ip: self.src_ip, body: self.body.reply() }
            }
        }

        pub trait TransportPacketExt<I: IpExt>:
            MaybeTransportPacket + MaybeIcmpErrorPayload<I>
        {
            fn proto() -> Option<I::Proto>;
            fn len(&self) -> usize;
        }

        impl<I: FilterIpExt, T> IpPacket<I> for FakeIpPacket<I, T>
        where
            for<'a> &'a T: TransportPacketExt<I>,
            for<'a> &'a mut T: MaybeTransportPacketMut<I> + MaybeIcmpErrorMut<I>,
        {
            type TransportPacket<'a>
                = &'a T
            where
                T: 'a;
            type TransportPacketMut<'a>
                = &'a mut T
            where
                T: 'a;
            type IcmpError<'a>
                = &'a T
            where
                T: 'a;
            type IcmpErrorMut<'a>
                = &'a mut T
            where
                T: 'a;

            fn src_addr(&self) -> I::Addr {
                self.src_ip
            }

            fn set_src_addr(&mut self, addr: I::Addr) {
                self.src_ip = addr;
            }

            fn dst_addr(&self) -> I::Addr {
                self.dst_ip
            }

            fn set_dst_addr(&mut self, addr: I::Addr) {
                self.dst_ip = addr;
            }

            fn protocol(&self) -> Option<I::Proto> {
                <&T>::proto()
            }

            fn maybe_transport_packet(&self) -> Self::TransportPacket<'_> {
                &self.body
            }

            fn transport_packet_mut(&mut self) -> Self::TransportPacketMut<'_> {
                &mut self.body
            }

            fn maybe_icmp_error<'a>(&'a self) -> Self::IcmpError<'a> {
                &self.body
            }

            fn icmp_error_mut<'a>(&'a mut self) -> Self::IcmpErrorMut<'a> {
                &mut self.body
            }
        }

        impl<I: TestIpExt, T> PartialSerializer<NetworkSerializationContext> for FakeIpPacket<I, T>
        where
            for<'a> &'a T: TransportPacketExt<I>,
        {
            fn partial_serialize_new_buf<B: GrowBufferMut, A: LayoutBufferAlloc<B>>(
                &self,
                context: &mut NetworkSerializationContext,
                constraints: PacketConstraints,
                alloc: A,
            ) -> Result<(B, usize), SerializeError<A::Error>> {
                assert!(constraints == PacketConstraints::UNCONSTRAINED);

                let Some(proto) = <&T>::proto() else {
                    let buffer = alloc.layout_alloc(0, 0, 0)?;
                    return Ok((buffer, 0));
                };
                let builder = I::PacketBuilder::new(self.src_ip, self.dst_ip, I::PACKET_TTL, proto);
                let constraints = builder.constraints();
                let header_len = constraints.header_len();
                let body_len = (&self.body).len();

                let mut buffer = alloc.layout_alloc(header_len, 0, 0)?;
                buffer.with_parts_mut(|prefix, _body, _suffix| {
                    builder.partial_serialize(context, body_len, prefix);
                });

                Ok((buffer, header_len + body_len))
            }
        }

        #[derive(Clone, Debug, PartialEq)]
        pub struct FakeTcpSegment {
            pub src_port: u16,
            pub dst_port: u16,
            pub segment: SegmentHeader,
            pub payload_len: usize,
        }

        impl<I: FilterIpExt> TransportPacketExt<I> for &FakeTcpSegment {
            fn proto() -> Option<I::Proto> {
                Some(I::map_ip_out(
                    (),
                    |()| Ipv4Proto::Proto(IpProto::Tcp),
                    |()| Ipv6Proto::Proto(IpProto::Tcp),
                ))
            }

            fn len(&self) -> usize {
                packet_formats::tcp::HDR_PREFIX_LEN + self.payload_len
            }
        }

        impl MaybeTransportPacket for &FakeTcpSegment {
            fn transport_packet_data(&self) -> Option<TransportPacketData> {
                Some(TransportPacketData::Tcp {
                    src_port: self.src_port,
                    dst_port: self.dst_port,
                    segment: self.segment.clone(),
                    payload_len: self.payload_len,
                })
            }
        }

        impl<I: IpExt> MaybeTransportPacketMut<I> for FakeTcpSegment {
            type TransportPacketMut<'a> = &'a mut Self;

            fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
                Some(self)
            }
        }

        impl<I: IpExt> TransportPacketMut<I> for FakeTcpSegment {
            fn set_src_port(&mut self, port: NonZeroU16) {
                self.src_port = port.get();
            }

            fn set_dst_port(&mut self, port: NonZeroU16) {
                self.dst_port = port.get();
            }

            fn update_pseudo_header_src_addr(&mut self, _: I::Addr, _: I::Addr) {}

            fn update_pseudo_header_dst_addr(&mut self, _: I::Addr, _: I::Addr) {}
        }

        impl<I: IpExt> MaybeIcmpErrorPayload<I> for FakeTcpSegment {
            fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
                None
            }
        }

        impl<I: FilterIpExt> MaybeIcmpErrorMut<I> for FakeTcpSegment {
            type IcmpErrorMut<'a>
                = Never
            where
                Self: 'a;

            fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
                None
            }
        }

        #[derive(Clone, Debug, PartialEq)]
        pub struct FakeUdpPacket {
            pub src_port: u16,
            pub dst_port: u16,
        }

        impl FakeUdpPacket {
            const PAYLOAD_LEN: usize = 4;

            fn reply(&self) -> Self {
                Self { src_port: self.dst_port, dst_port: self.src_port }
            }
        }

        impl<I: FilterIpExt> TransportPacketExt<I> for &FakeUdpPacket {
            fn proto() -> Option<I::Proto> {
                Some(I::map_ip_out(
                    (),
                    |()| Ipv4Proto::Proto(IpProto::Udp),
                    |()| Ipv6Proto::Proto(IpProto::Udp),
                ))
            }

            fn len(&self) -> usize {
                packet_formats::udp::HEADER_BYTES + FakeUdpPacket::PAYLOAD_LEN
            }
        }

        impl MaybeTransportPacket for &FakeUdpPacket {
            fn transport_packet_data(&self) -> Option<TransportPacketData> {
                Some(TransportPacketData::Generic {
                    src_port: self.src_port,
                    dst_port: self.dst_port,
                })
            }
        }

        impl<I: IpExt> MaybeTransportPacketMut<I> for FakeUdpPacket {
            type TransportPacketMut<'a> = &'a mut Self;

            fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
                Some(self)
            }
        }

        impl<I: IpExt> TransportPacketMut<I> for FakeUdpPacket {
            fn set_src_port(&mut self, port: NonZeroU16) {
                self.src_port = port.get();
            }

            fn set_dst_port(&mut self, port: NonZeroU16) {
                self.dst_port = port.get();
            }

            fn update_pseudo_header_src_addr(&mut self, _: I::Addr, _: I::Addr) {}

            fn update_pseudo_header_dst_addr(&mut self, _: I::Addr, _: I::Addr) {}
        }

        impl<I: IpExt> MaybeIcmpErrorPayload<I> for FakeUdpPacket {
            fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
                None
            }
        }

        impl<I: FilterIpExt> MaybeIcmpErrorMut<I> for FakeUdpPacket {
            type IcmpErrorMut<'a>
                = Never
            where
                Self: 'a;

            fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
                None
            }
        }

        #[derive(Clone, Debug, PartialEq)]
        pub struct FakeNullPacket;

        impl<I: IpExt> TransportPacketExt<I> for &FakeNullPacket {
            fn proto() -> Option<I::Proto> {
                None
            }

            fn len(&self) -> usize {
                0
            }
        }

        impl MaybeTransportPacket for &FakeNullPacket {
            fn transport_packet_data(&self) -> Option<TransportPacketData> {
                None
            }
        }

        impl<I: IpExt> MaybeTransportPacketMut<I> for FakeNullPacket {
            type TransportPacketMut<'a> = Never;

            fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
                None
            }
        }

        impl<I: IpExt> MaybeIcmpErrorPayload<I> for FakeNullPacket {
            fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
                None
            }
        }

        impl<I: FilterIpExt> MaybeIcmpErrorMut<I> for FakeNullPacket {
            type IcmpErrorMut<'a>
                = Never
            where
                Self: 'a;

            fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
                None
            }
        }

        pub struct FakeIcmpEchoRequest {
            pub id: u16,
        }

        impl<I: FilterIpExt> TransportPacketExt<I> for &FakeIcmpEchoRequest {
            fn proto() -> Option<I::Proto> {
                Some(I::map_ip_out((), |()| Ipv4Proto::Icmp, |()| Ipv6Proto::Icmpv6))
            }

            fn len(&self) -> usize {
                // ICMP header is 8 bytes.
                8
            }
        }

        impl MaybeTransportPacket for &FakeIcmpEchoRequest {
            fn transport_packet_data(&self) -> Option<TransportPacketData> {
                Some(TransportPacketData::Generic { src_port: self.id, dst_port: 0 })
            }
        }

        impl<I: IpExt> MaybeTransportPacketMut<I> for FakeIcmpEchoRequest {
            type TransportPacketMut<'a> = &'a mut Self;

            fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
                Some(self)
            }
        }

        impl<I: IpExt> TransportPacketMut<I> for FakeIcmpEchoRequest {
            fn set_src_port(&mut self, port: NonZeroU16) {
                self.id = port.get();
            }

            fn set_dst_port(&mut self, _: NonZeroU16) {
                panic!("cannot set destination port for ICMP echo request")
            }

            fn update_pseudo_header_src_addr(&mut self, _: I::Addr, _: I::Addr) {}

            fn update_pseudo_header_dst_addr(&mut self, _: I::Addr, _: I::Addr) {}
        }

        impl<I: IpExt> MaybeIcmpErrorPayload<I> for FakeIcmpEchoRequest {
            fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
                None
            }
        }

        impl<I: FilterIpExt> MaybeIcmpErrorMut<I> for FakeIcmpEchoRequest {
            type IcmpErrorMut<'a>
                = Never
            where
                Self: 'a;

            fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
                None
            }
        }

        pub trait ArbitraryValue {
            fn arbitrary_value() -> Self;
        }

        impl<I, T> ArbitraryValue for FakeIpPacket<I, T>
        where
            I: TestIpExt,
            T: ArbitraryValue,
            for<'a> &'a T: TransportPacketExt<I>,
        {
            fn arbitrary_value() -> Self {
                FakeIpPacket { src_ip: I::SRC_IP, dst_ip: I::DST_IP, body: T::arbitrary_value() }
            }
        }

        impl ArbitraryValue for FakeTcpSegment {
            fn arbitrary_value() -> Self {
                FakeTcpSegment {
                    src_port: 33333,
                    dst_port: 44444,
                    segment: SegmentHeader::arbitrary_value(),
                    payload_len: 8888,
                }
            }
        }

        impl ArbitraryValue for FakeUdpPacket {
            fn arbitrary_value() -> Self {
                FakeUdpPacket { src_port: 33333, dst_port: 44444 }
            }
        }

        impl ArbitraryValue for FakeNullPacket {
            fn arbitrary_value() -> Self {
                FakeNullPacket
            }
        }

        impl ArbitraryValue for FakeIcmpEchoRequest {
            fn arbitrary_value() -> Self {
                FakeIcmpEchoRequest { id: 1 }
            }
        }

        impl ArbitraryValue for SegmentHeader {
            fn arbitrary_value() -> Self {
                SegmentHeader {
                    seq: SeqNum::new(55555),
                    wnd: UnscaledWindowSize::from(1234),
                    ..Default::default()
                }
            }
        }

        pub(crate) trait IcmpErrorMessage<I: FilterIpExt> {
            type Serializer: TransportPacketSerializer<I, Buffer: packet::ReusableBuffer>
                + Debug
                + PartialEq;

            fn proto() -> I::Proto {
                I::map_ip((), |()| Ipv4Proto::Icmp, |()| Ipv6Proto::Icmpv6)
            }

            fn make_serializer(
                src_ip: I::Addr,
                dst_ip: I::Addr,
                inner: Vec<u8>,
            ) -> Self::Serializer;

            fn make_serializer_truncated(
                src_ip: I::Addr,
                dst_ip: I::Addr,
                mut payload: Vec<u8>,
                truncate_payload: Option<usize>,
            ) -> Self::Serializer {
                if let Some(len) = truncate_payload {
                    payload.truncate(len);
                }

                Self::make_serializer(src_ip, dst_ip, payload)
            }
        }

        pub(crate) struct Icmpv4DestUnreachableError;

        impl IcmpErrorMessage<Ipv4> for Icmpv4DestUnreachableError {
            type Serializer = Nested<Buf<Vec<u8>>, IcmpPacketBuilder<Ipv4, IcmpDestUnreachable>>;

            fn make_serializer(
                src_ip: Ipv4Addr,
                dst_ip: Ipv4Addr,
                payload: Vec<u8>,
            ) -> Self::Serializer {
                IcmpPacketBuilder::<Ipv4, IcmpDestUnreachable>::new(
                    src_ip,
                    dst_ip,
                    Icmpv4DestUnreachableCode::DestHostUnreachable,
                    IcmpDestUnreachable::default(),
                )
                .wrap_body(Buf::new(payload, ..))
            }
        }

        pub(crate) struct Icmpv6DestUnreachableError;

        impl IcmpErrorMessage<Ipv6> for Icmpv6DestUnreachableError {
            type Serializer = Nested<
                TruncatingSerializer<Buf<Vec<u8>>>,
                IcmpPacketBuilder<Ipv6, IcmpDestUnreachable>,
            >;

            fn make_serializer(
                src_ip: Ipv6Addr,
                dst_ip: Ipv6Addr,
                payload: Vec<u8>,
            ) -> Self::Serializer {
                IcmpPacketBuilder::<Ipv6, IcmpDestUnreachable>::new(
                    src_ip,
                    dst_ip,
                    Icmpv6DestUnreachableCode::AddrUnreachable,
                    IcmpDestUnreachable::default(),
                )
                .wrap_body(TruncatingSerializer::new(
                    Buf::new(payload, ..),
                    TruncateDirection::DiscardBack,
                ))
            }
        }
    }

    /// Creates a new `IpPacket` with the specified addresses and body.
    pub fn new_filter_egress_ip_packet<I: FilterIpExt, S: TransportPacketSerializer<I>>(
        src_addr: I::Addr,
        dst_addr: I::Addr,
        protocol: I::Proto,
        body: &'_ mut S,
    ) -> impl FilterIpPacket<I> + use<'_, I, S> {
        TxPacket::new(src_addr, dst_addr, protocol, body)
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::fmt::Debug;
    use core::marker::PhantomData;
    use netstack3_base::{NetworkSerializationContext, SeqNum, UnscaledWindowSize};

    use assert_matches::assert_matches;
    use ip_test_macro::ip_test;
    use packet::{
        EmptyBuf, FragmentedBuffer as _, InnerPacketBuilder as _, ParseBufferMut, PartialSerializer,
    };
    use packet_formats::icmp::IcmpZeroCode;
    use packet_formats::tcp::TcpSegmentBuilder;
    use test_case::{test_case, test_matrix};

    use crate::conntrack;

    use super::testutil::internal::{
        IcmpErrorMessage, Icmpv4DestUnreachableError, Icmpv6DestUnreachableError, TestIpExt,
    };
    use super::*;

    const SRC_PORT: NonZeroU16 = NonZeroU16::new(11111).unwrap();
    const DST_PORT: NonZeroU16 = NonZeroU16::new(22222).unwrap();
    const SRC_PORT_2: NonZeroU16 = NonZeroU16::new(44444).unwrap();
    const DST_PORT_2: NonZeroU16 = NonZeroU16::new(55555).unwrap();

    const SEQ_NUM: u32 = 1;
    const ACK_NUM: Option<u32> = Some(2);
    const WINDOW_SIZE: u16 = 3u16;

    trait Protocol {
        const HEADER_SIZE: usize;

        type Serializer<'a, I: FilterIpExt>: TransportPacketSerializer<I, Buffer: packet::ReusableBuffer>
            + MaybeTransportPacketMut<I>
            + Debug
            + PartialEq;

        fn proto<I: IpExt>() -> I::Proto;

        fn make_serializer_with_ports_data<'a, I: FilterIpExt>(
            src_ip: I::Addr,
            dst_ip: I::Addr,
            src_port: NonZeroU16,
            dst_port: NonZeroU16,
            data: &'a [u8],
        ) -> Self::Serializer<'a, I>;

        fn make_serializer_with_ports<'a, I: FilterIpExt>(
            src_ip: I::Addr,
            dst_ip: I::Addr,
            src_port: NonZeroU16,
            dst_port: NonZeroU16,
        ) -> Self::Serializer<'a, I> {
            Self::make_serializer_with_ports_data(src_ip, dst_ip, src_port, dst_port, &[1, 2, 3])
        }

        fn make_serializer<'a, I: FilterIpExt>(
            src_ip: I::Addr,
            dst_ip: I::Addr,
        ) -> Self::Serializer<'a, I> {
            Self::make_serializer_with_ports(src_ip, dst_ip, SRC_PORT, DST_PORT)
        }

        fn make_packet<I: FilterIpExt>(src_ip: I::Addr, dst_ip: I::Addr) -> Vec<u8> {
            Self::make_packet_with_ports::<I>(src_ip, dst_ip, SRC_PORT, DST_PORT)
        }

        fn make_packet_with_ports<I: FilterIpExt>(
            src_ip: I::Addr,
            dst_ip: I::Addr,
            src_port: NonZeroU16,
            dst_port: NonZeroU16,
        ) -> Vec<u8> {
            Self::make_serializer_with_ports::<I>(src_ip, dst_ip, src_port, dst_port)
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .expect("serialize packet")
                .unwrap_b()
                .into_inner()
        }

        fn make_ip_packet_with_ports_data<I: FilterIpExt>(
            src_ip: I::Addr,
            dst_ip: I::Addr,
            src_port: NonZeroU16,
            dst_port: NonZeroU16,
            data: &[u8],
        ) -> Vec<u8> {
            I::PacketBuilder::new(src_ip, dst_ip, u8::MAX, Self::proto::<I>())
                .wrap_body(Self::make_serializer_with_ports_data::<I>(
                    src_ip, dst_ip, src_port, dst_port, data,
                ))
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .expect("serialize packet")
                .unwrap_b()
                .into_inner()
        }
    }

    struct Udp;

    impl Protocol for Udp {
        const HEADER_SIZE: usize = 8;

        type Serializer<'a, I: FilterIpExt> =
            Nested<InnerSerializer<&'a [u8], EmptyBuf>, UdpPacketBuilder<I::Addr>>;

        fn proto<I: IpExt>() -> I::Proto {
            IpProto::Udp.into()
        }

        fn make_serializer_with_ports_data<'a, I: FilterIpExt>(
            src_ip: I::Addr,
            dst_ip: I::Addr,
            src_port: NonZeroU16,
            dst_port: NonZeroU16,
            data: &'a [u8],
        ) -> Self::Serializer<'a, I> {
            UdpPacketBuilder::new(src_ip, dst_ip, Some(src_port), dst_port)
                .wrap_body(data.into_serializer())
        }
    }

    // The `TcpSegmentBuilder` impls are test-only on purpose, and removing this
    // restriction should be thought through.
    //
    // TCP state tracking depends on being able to read TCP options, but
    // TcpSegmentBuilder does not have this information. If a TcpSegmentBuilder
    // passes through filtering with options tracked separately, then these will
    // not be seen by conntrack and could lead to state desynchronization.
    impl<A: IpAddress, Inner: PayloadLen> MaybeTransportPacket for Nested<Inner, TcpSegmentBuilder<A>> {
        fn transport_packet_data(&self) -> Option<TransportPacketData> {
            Some(TransportPacketData::Tcp {
                src_port: TcpSegmentBuilder::src_port(self.outer()).map_or(0, NonZeroU16::get),
                dst_port: TcpSegmentBuilder::dst_port(self.outer()).map_or(0, NonZeroU16::get),
                segment: self.outer().try_into().ok()?,
                payload_len: self.inner().len(),
            })
        }
    }

    impl<I: IpExt, Inner> MaybeTransportPacketMut<I> for Nested<Inner, TcpSegmentBuilder<I::Addr>> {
        type TransportPacketMut<'a>
            = &'a mut Self
        where
            Self: 'a;

        fn transport_packet_mut(&mut self) -> Option<Self::TransportPacketMut<'_>> {
            Some(self)
        }
    }

    impl<I: IpExt, Inner> TransportPacketMut<I> for Nested<Inner, TcpSegmentBuilder<I::Addr>> {
        fn set_src_port(&mut self, port: NonZeroU16) {
            self.outer_mut().set_src_port(port);
        }

        fn set_dst_port(&mut self, port: NonZeroU16) {
            self.outer_mut().set_dst_port(port);
        }

        fn update_pseudo_header_src_addr(&mut self, _old: I::Addr, new: I::Addr) {
            self.outer_mut().set_src_ip(new);
        }

        fn update_pseudo_header_dst_addr(&mut self, _old: I::Addr, new: I::Addr) {
            self.outer_mut().set_dst_ip(new);
        }
    }

    impl<A: IpAddress, I: IpExt, Inner> MaybeIcmpErrorPayload<I>
        for Nested<Inner, TcpSegmentBuilder<A>>
    {
        fn icmp_error_payload(&self) -> Option<ParsedIcmpErrorPayload<I>> {
            None
        }
    }

    impl<A: IpAddress, I: FilterIpExt, Inner> MaybeIcmpErrorMut<I>
        for Nested<Inner, TcpSegmentBuilder<A>>
    {
        type IcmpErrorMut<'a>
            = Never
        where
            Self: 'a;

        fn icmp_error_mut<'a>(&'a mut self) -> Option<Self::IcmpErrorMut<'a>> {
            None
        }
    }

    enum Tcp {}

    impl Protocol for Tcp {
        const HEADER_SIZE: usize = 20;

        type Serializer<'a, I: FilterIpExt> =
            Nested<InnerSerializer<&'a [u8], EmptyBuf>, TcpSegmentBuilder<I::Addr>>;

        fn proto<I: IpExt>() -> I::Proto {
            IpProto::Tcp.into()
        }

        fn make_serializer_with_ports_data<'a, I: FilterIpExt>(
            src_ip: I::Addr,
            dst_ip: I::Addr,
            src_port: NonZeroU16,
            dst_port: NonZeroU16,
            data: &'a [u8],
        ) -> Self::Serializer<'a, I> {
            TcpSegmentBuilder::new(
                src_ip,
                dst_ip,
                src_port,
                dst_port,
                SEQ_NUM,
                ACK_NUM,
                WINDOW_SIZE,
            )
            .wrap_body(data.into_serializer())
        }
    }

    enum IcmpEchoRequest {}

    impl Protocol for IcmpEchoRequest {
        const HEADER_SIZE: usize = 8;

        type Serializer<'a, I: FilterIpExt> = Nested<
            InnerSerializer<&'a [u8], EmptyBuf>,
            IcmpPacketBuilder<I, icmp::IcmpEchoRequest>,
        >;

        fn proto<I: IpExt>() -> I::Proto {
            I::map_ip((), |()| Ipv4Proto::Icmp, |()| Ipv6Proto::Icmpv6)
        }

        fn make_serializer_with_ports_data<'a, I: FilterIpExt>(
            src_ip: I::Addr,
            dst_ip: I::Addr,
            src_port: NonZeroU16,
            _dst_port: NonZeroU16,
            data: &'a [u8],
        ) -> Self::Serializer<'a, I> {
            IcmpPacketBuilder::<I, _>::new(
                src_ip,
                dst_ip,
                IcmpZeroCode,
                icmp::IcmpEchoRequest::new(/* id */ src_port.get(), /* seq */ 0),
            )
            .wrap_body(data.into_serializer())
        }
    }

    enum IcmpEchoReply {}

    impl Protocol for IcmpEchoReply {
        const HEADER_SIZE: usize = 8;

        type Serializer<'a, I: FilterIpExt> =
            Nested<InnerSerializer<&'a [u8], EmptyBuf>, IcmpPacketBuilder<I, icmp::IcmpEchoReply>>;

        fn proto<I: IpExt>() -> I::Proto {
            I::map_ip((), |()| Ipv4Proto::Icmp, |()| Ipv6Proto::Icmpv6)
        }

        fn make_serializer_with_ports_data<'a, I: FilterIpExt>(
            src_ip: I::Addr,
            dst_ip: I::Addr,
            _src_port: NonZeroU16,
            dst_port: NonZeroU16,
            data: &'a [u8],
        ) -> Self::Serializer<'a, I> {
            IcmpPacketBuilder::<I, _>::new(
                src_ip,
                dst_ip,
                IcmpZeroCode,
                icmp::IcmpEchoReply::new(/* id */ dst_port.get(), /* seq */ 0),
            )
            .wrap_body(data.into_serializer())
        }
    }

    enum TransportPacketDataProtocol {
        Tcp,
        Udp,
        IcmpEchoRequest,
    }

    impl TransportPacketDataProtocol {
        fn make_packet<I: TestIpExt>(&self, src_ip: I::Addr, dst_ip: I::Addr) -> Vec<u8> {
            match self {
                TransportPacketDataProtocol::Tcp => Tcp::make_packet::<I>(src_ip, dst_ip),
                TransportPacketDataProtocol::Udp => Udp::make_packet::<I>(src_ip, dst_ip),
                TransportPacketDataProtocol::IcmpEchoRequest => {
                    IcmpEchoRequest::make_packet::<I>(src_ip, dst_ip)
                }
            }
        }

        fn make_ip_packet_with_ports_data<I: TestIpExt>(
            &self,
            src_ip: I::Addr,
            dst_ip: I::Addr,
            src_port: NonZeroU16,
            dst_port: NonZeroU16,
            data: &[u8],
        ) -> Vec<u8> {
            match self {
                TransportPacketDataProtocol::Tcp => Tcp::make_ip_packet_with_ports_data::<I>(
                    src_ip, dst_ip, src_port, dst_port, data,
                ),
                TransportPacketDataProtocol::Udp => Udp::make_ip_packet_with_ports_data::<I>(
                    src_ip, dst_ip, src_port, dst_port, data,
                ),
                TransportPacketDataProtocol::IcmpEchoRequest => {
                    IcmpEchoRequest::make_ip_packet_with_ports_data::<I>(
                        src_ip, dst_ip, src_port, dst_port, data,
                    )
                }
            }
        }

        fn proto<I: TestIpExt>(&self) -> I::Proto {
            match self {
                TransportPacketDataProtocol::Tcp => Tcp::proto::<I>(),
                TransportPacketDataProtocol::Udp => Udp::proto::<I>(),
                TransportPacketDataProtocol::IcmpEchoRequest => IcmpEchoRequest::proto::<I>(),
            }
        }
    }

    #[ip_test(I)]
    #[test_case(TransportPacketDataProtocol::Udp)]
    #[test_case(TransportPacketDataProtocol::Tcp)]
    #[test_case(TransportPacketDataProtocol::IcmpEchoRequest)]
    fn transport_packet_data_from_serialized<I: TestIpExt>(proto: TransportPacketDataProtocol) {
        let expected_data = match proto {
            TransportPacketDataProtocol::Tcp => TransportPacketData::Tcp {
                src_port: SRC_PORT.get(),
                dst_port: DST_PORT.get(),
                segment: SegmentHeader {
                    seq: SeqNum::new(SEQ_NUM),
                    ack: ACK_NUM.map(SeqNum::new),
                    wnd: UnscaledWindowSize::from(WINDOW_SIZE),
                    ..Default::default()
                },
                payload_len: 3,
            },
            TransportPacketDataProtocol::Udp => {
                TransportPacketData::Generic { src_port: SRC_PORT.get(), dst_port: DST_PORT.get() }
            }
            TransportPacketDataProtocol::IcmpEchoRequest => {
                TransportPacketData::Generic { src_port: SRC_PORT.get(), dst_port: SRC_PORT.get() }
            }
        };

        let buf = proto.make_packet::<I>(I::SRC_IP, I::DST_IP);
        let parsed_data = TransportPacketData::parse_in_ip_packet::<I, _>(
            I::SRC_IP,
            I::DST_IP,
            proto.proto::<I>(),
            buf.as_slice(),
        )
        .expect("failed to parse transport packet data");

        assert_eq!(parsed_data, expected_data);
    }

    // Regression test for https://fxbug.dev/518696592.
    // Verifies that we still extract port information (as Tcp packet data)
    // even if TCP options parsing fails due to malformed options.
    #[ip_test(I)]
    fn transport_packet_data_from_serialized_invalid_tcp_options<I: TestIpExt>() {
        let mut buf = TransportPacketDataProtocol::Tcp.make_packet::<I>(I::SRC_IP, I::DST_IP);

        // Normal TCP header is 20 bytes.
        assert!(buf.len() >= 20);

        // data_offset is in buf[12], most significant 4 bits.
        // Change data_offset from 5 (20 bytes) to 6 (24 bytes) to make room for options.
        buf[12] = (6 << 4) | (buf[12] & 0x0F);

        // Modify TCP header to include an invalid option [255, 0, 0, 0] at index 20.
        let mut new_buf = Vec::new();
        new_buf.extend_from_slice(&buf[..20]);
        new_buf.extend_from_slice(&[255, 0, 0, 0]);
        new_buf.extend_from_slice(&buf[20..]);

        let parsed_data = TransportPacketData::parse_in_ip_packet::<I, _>(
            I::SRC_IP,
            I::DST_IP,
            IpProto::Tcp.into(),
            new_buf.as_slice(),
        );

        assert_matches!(
            parsed_data,
            Some(TransportPacketData::Tcp { src_port, dst_port, .. }) => {
                assert_eq!(src_port, SRC_PORT.get());
                assert_eq!(dst_port, DST_PORT.get());
            }
        );
    }

    // Regression test for https://fxbug.dev/518696592.
    // Verifies that we still extract port information (as Generic packet data)
    // even if TCP header parsing fails due to malformed (mutually exclusive) flags.
    #[ip_test(I)]
    fn transport_packet_data_from_serialized_malformed_tcp_flags<I: TestIpExt>() {
        let mut buf = TransportPacketDataProtocol::Tcp.make_packet::<I>(I::SRC_IP, I::DST_IP);

        // Modify TCP header to include mutually exclusive flags: SYN and RST.
        // Normal TCP header is 20 bytes.
        assert!(buf.len() >= 20);

        // Flags are in buf[13].
        // SYN is 0x02, RST is 0x04. Set both.
        buf[13] |= 0x02 | 0x04;

        let parsed_data = TransportPacketData::parse_in_ip_packet::<I, _>(
            I::SRC_IP,
            I::DST_IP,
            IpProto::Tcp.into(),
            buf.as_slice(),
        );

        assert_matches!(
            parsed_data,
            Some(TransportPacketData::Generic { src_port, dst_port }) => {
                assert_eq!(src_port, SRC_PORT.get());
                assert_eq!(dst_port, DST_PORT.get());
            }
        );
    }

    enum PacketType {
        FullyParsed,
        Raw,
    }

    #[ip_test(I)]
    #[test_matrix(
        [
            TransportPacketDataProtocol::Udp,
            TransportPacketDataProtocol::Tcp,
            TransportPacketDataProtocol::IcmpEchoRequest,
        ],
        [
            PacketType::FullyParsed,
            PacketType::Raw
        ]
    )]
    fn conntrack_packet_data_from_ip_packet<I: TestIpExt>(
        proto: TransportPacketDataProtocol,
        packet_type: PacketType,
    ) where
        for<'a> I::Packet<&'a mut [u8]>: IpPacket<I>,
        for<'a> I::PacketRaw<&'a mut [u8]>: IpPacket<I>,
    {
        let expected_data = match proto {
            TransportPacketDataProtocol::Tcp => conntrack::PacketMetadata::new(
                I::SRC_IP,
                I::DST_IP,
                conntrack::TransportProtocol::Tcp,
                TransportPacketData::Tcp {
                    src_port: SRC_PORT.get(),
                    dst_port: DST_PORT.get(),
                    segment: SegmentHeader {
                        seq: SeqNum::new(SEQ_NUM),
                        ack: ACK_NUM.map(SeqNum::new),
                        wnd: UnscaledWindowSize::from(WINDOW_SIZE),
                        ..Default::default()
                    },
                    payload_len: 3,
                },
            ),
            TransportPacketDataProtocol::Udp => conntrack::PacketMetadata::new(
                I::SRC_IP,
                I::DST_IP,
                conntrack::TransportProtocol::Udp,
                TransportPacketData::Generic { src_port: SRC_PORT.get(), dst_port: DST_PORT.get() },
            ),
            TransportPacketDataProtocol::IcmpEchoRequest => conntrack::PacketMetadata::new(
                I::SRC_IP,
                I::DST_IP,
                conntrack::TransportProtocol::Icmp,
                TransportPacketData::Generic { src_port: SRC_PORT.get(), dst_port: SRC_PORT.get() },
            ),
        };

        let mut buf = proto.make_ip_packet_with_ports_data::<I>(
            I::SRC_IP,
            I::DST_IP,
            SRC_PORT,
            DST_PORT,
            &[1, 2, 3],
        );

        let parsed_data = match packet_type {
            PacketType::FullyParsed => {
                let packet = I::Packet::parse_mut(SliceBufViewMut::new(buf.as_mut()), ())
                    .expect("parse IP packet");
                packet.conntrack_packet().expect("packet should be trackable")
            }
            PacketType::Raw => {
                let packet = I::PacketRaw::parse_mut(SliceBufViewMut::new(buf.as_mut()), ())
                    .expect("parse IP packet");
                packet.conntrack_packet().expect("packet should be trackable")
            }
        };

        assert_eq!(parsed_data, expected_data);
    }

    #[ip_test(I)]
    #[test_case(PhantomData::<Udp>)]
    #[test_case(PhantomData::<Tcp>)]
    #[test_case(PhantomData::<IcmpEchoRequest>)]
    fn update_pseudo_header_address_updates_checksum<I: TestIpExt, P: Protocol>(
        _proto: PhantomData<P>,
    ) {
        let mut buf = P::make_packet::<I>(I::SRC_IP, I::DST_IP);
        let view = SliceBufViewMut::new(&mut buf);

        let mut packet = ParsedTransportHeaderMut::<I>::parse_in_ip_packet(P::proto::<I>(), view)
            .expect("parse transport header");
        packet.update_pseudo_header_src_addr(I::SRC_IP, I::SRC_IP_2);
        packet.update_pseudo_header_dst_addr(I::DST_IP, I::DST_IP_2);
        // Drop the packet because it's holding a mutable borrow of `buf` which
        // we need to assert equality later.
        drop(packet);

        let equivalent = P::make_packet::<I>(I::SRC_IP_2, I::DST_IP_2);

        assert_eq!(equivalent, buf);
    }

    #[ip_test(I)]
    #[test_case(PhantomData::<Udp>, true, true)]
    #[test_case(PhantomData::<Tcp>, true, true)]
    #[test_case(PhantomData::<IcmpEchoRequest>, true, false)]
    #[test_case(PhantomData::<IcmpEchoReply>, false, true)]
    fn parsed_packet_update_src_dst_port_updates_checksum<I: TestIpExt, P: Protocol>(
        _proto: PhantomData<P>,
        update_src_port: bool,
        update_dst_port: bool,
    ) {
        let mut buf = P::make_packet_with_ports::<I>(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT);
        let view = SliceBufViewMut::new(&mut buf);

        let mut packet = ParsedTransportHeaderMut::<I>::parse_in_ip_packet(P::proto::<I>(), view)
            .expect("parse transport header");
        let expected_src_port = if update_src_port {
            packet.set_src_port(SRC_PORT_2);
            SRC_PORT_2
        } else {
            SRC_PORT
        };
        let expected_dst_port = if update_dst_port {
            packet.set_dst_port(DST_PORT_2);
            DST_PORT_2
        } else {
            DST_PORT
        };
        drop(packet);

        let equivalent = P::make_packet_with_ports::<I>(
            I::SRC_IP,
            I::DST_IP,
            expected_src_port,
            expected_dst_port,
        );

        assert_eq!(equivalent, buf);
    }

    #[ip_test(I)]
    #[test_case(PhantomData::<Udp>)]
    #[test_case(PhantomData::<Tcp>)]
    fn serializer_update_src_dst_port_updates_checksum<I: TestIpExt, P: Protocol>(
        _proto: PhantomData<P>,
    ) {
        let mut serializer =
            P::make_serializer_with_ports::<I>(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT);
        let mut packet =
            serializer.transport_packet_mut().expect("packet should support rewriting");
        packet.set_src_port(SRC_PORT_2);
        packet.set_dst_port(DST_PORT_2);
        drop(packet);

        let equivalent =
            P::make_serializer_with_ports::<I>(I::SRC_IP, I::DST_IP, SRC_PORT_2, DST_PORT_2);

        assert_eq!(equivalent, serializer);
    }

    #[ip_test(I)]
    fn icmp_echo_request_update_id_port_updates_checksum<I: TestIpExt>() {
        let mut serializer = IcmpPacketBuilder::<I, _>::new(
            I::SRC_IP,
            I::DST_IP,
            IcmpZeroCode,
            icmp::IcmpEchoRequest::new(SRC_PORT.get(), /* seq */ 0),
        )
        .wrap_body(EmptyBuf);
        serializer
            .transport_packet_mut()
            .expect("packet should support rewriting")
            .set_src_port(SRC_PORT_2);

        let equivalent = IcmpPacketBuilder::<I, _>::new(
            I::SRC_IP,
            I::DST_IP,
            IcmpZeroCode,
            icmp::IcmpEchoRequest::new(SRC_PORT_2.get(), /* seq */ 0),
        )
        .wrap_body(EmptyBuf);

        assert_eq!(equivalent, serializer);
    }

    #[ip_test(I)]
    fn icmp_echo_reply_update_id_port_updates_checksum<I: TestIpExt>() {
        let mut serializer = IcmpPacketBuilder::<I, _>::new(
            I::SRC_IP,
            I::DST_IP,
            IcmpZeroCode,
            icmp::IcmpEchoReply::new(SRC_PORT.get(), /* seq */ 0),
        )
        .wrap_body(EmptyBuf);
        serializer
            .transport_packet_mut()
            .expect("packet should support rewriting")
            .set_dst_port(SRC_PORT_2);

        let equivalent = IcmpPacketBuilder::<I, _>::new(
            I::SRC_IP,
            I::DST_IP,
            IcmpZeroCode,
            icmp::IcmpEchoReply::new(SRC_PORT_2.get(), /* seq */ 0),
        )
        .wrap_body(EmptyBuf);

        assert_eq!(equivalent, serializer);
    }

    fn ip_packet<I: TestIpExt, P: Protocol>(src: I::Addr, dst: I::Addr) -> Buf<Vec<u8>> {
        Buf::new(P::make_packet::<I>(src, dst), ..)
            .wrap_in(I::PacketBuilder::new(src, dst, I::PACKET_TTL, P::proto::<I>()))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .expect("serialize IP packet")
            .unwrap_b()
    }

    #[ip_test(I)]
    #[test_matrix(
        [
            PhantomData::<Udp>,
            PhantomData::<Tcp>,
            PhantomData::<IcmpEchoRequest>,
        ],
        [
            PacketType::FullyParsed,
            PacketType::Raw
        ]
    )]
    fn ip_packet_set_src_dst_addr_updates_checksums<I: TestIpExt, P: Protocol>(
        _proto: PhantomData<P>,
        packet_type: PacketType,
    ) where
        for<'a> I::Packet<&'a mut [u8]>: IpPacket<I>,
        for<'a> I::PacketRaw<&'a mut [u8]>: IpPacket<I>,
    {
        let mut buf = ip_packet::<I, P>(I::SRC_IP, I::DST_IP).into_inner();

        match packet_type {
            PacketType::FullyParsed => {
                let mut packet = I::Packet::parse_mut(SliceBufViewMut::new(&mut buf), ())
                    .expect("parse IP packet");
                packet.set_src_addr(I::SRC_IP_2);
                packet.set_dst_addr(I::DST_IP_2);
            }
            PacketType::Raw => {
                let mut packet = I::PacketRaw::parse_mut(SliceBufViewMut::new(&mut buf), ())
                    .expect("parse IP packet");
                packet.set_src_addr(I::SRC_IP_2);
                packet.set_dst_addr(I::DST_IP_2);
            }
        }

        let equivalent = ip_packet::<I, P>(I::SRC_IP_2, I::DST_IP_2).into_inner();

        assert_eq!(equivalent, buf);
    }

    #[ip_test(I)]
    #[test_case(PhantomData::<Udp>)]
    #[test_case(PhantomData::<Tcp>)]
    #[test_case(PhantomData::<IcmpEchoRequest>)]
    fn forwarded_packet_set_src_dst_addr_updates_checksums<I: TestIpExt, P: Protocol>(
        _proto: PhantomData<P>,
    ) {
        let mut buffer = ip_packet::<I, P>(I::SRC_IP, I::DST_IP);
        let meta = buffer.parse::<I::Packet<_>>().expect("parse IP packet").parse_metadata();
        let mut packet = ForwardedPacket::<I, _>::new(
            I::SRC_IP,
            I::DST_IP,
            P::proto::<I>(),
            meta,
            buffer,
            false,
        );
        packet.set_src_addr(I::SRC_IP_2);
        packet.set_dst_addr(I::DST_IP_2);

        let mut buffer = ip_packet::<I, P>(I::SRC_IP_2, I::DST_IP_2);
        let meta = buffer.parse::<I::Packet<_>>().expect("parse IP packet").parse_metadata();
        let equivalent = ForwardedPacket::<I, _>::new(
            I::SRC_IP_2,
            I::DST_IP_2,
            P::proto::<I>(),
            meta,
            buffer,
            false,
        );

        assert_eq!(equivalent, packet);
    }

    #[ip_test(I)]
    #[test_case(PhantomData::<Udp>)]
    #[test_case(PhantomData::<Tcp>)]
    #[test_case(PhantomData::<IcmpEchoRequest>)]
    fn tx_packet_set_src_dst_addr_updates_checksums<I: TestIpExt, P: Protocol>(
        _proto: PhantomData<P>,
    ) {
        let mut body = P::make_serializer::<I>(I::SRC_IP, I::DST_IP);
        let mut packet = TxPacket::<I, _>::new(I::SRC_IP, I::DST_IP, P::proto::<I>(), &mut body);
        packet.set_src_addr(I::SRC_IP_2);
        packet.set_dst_addr(I::DST_IP_2);

        let mut equivalent_body = P::make_serializer::<I>(I::SRC_IP_2, I::DST_IP_2);
        let equivalent =
            TxPacket::new(I::SRC_IP_2, I::DST_IP_2, P::proto::<I>(), &mut equivalent_body);

        assert_eq!(equivalent, packet);
    }

    #[ip_test(I)]
    #[test_case(PhantomData::<Udp>)]
    #[test_case(PhantomData::<Tcp>)]
    #[test_case(PhantomData::<IcmpEchoRequest>)]
    fn nested_serializer_set_src_dst_addr_updates_checksums<I: TestIpExt, P: Protocol>(
        _proto: PhantomData<P>,
    ) {
        let mut packet =
            I::PacketBuilder::new(I::SRC_IP, I::DST_IP, I::PACKET_TTL, P::proto::<I>())
                .wrap_body(P::make_serializer::<I>(I::SRC_IP, I::DST_IP));
        packet.set_src_addr(I::SRC_IP_2);
        packet.set_dst_addr(I::DST_IP_2);

        let equivalent = P::make_serializer::<I>(I::SRC_IP_2, I::DST_IP_2).wrap_in(
            I::PacketBuilder::new(I::SRC_IP_2, I::DST_IP_2, I::PACKET_TTL, P::proto::<I>()),
        );

        assert_eq!(equivalent, packet);
    }

    #[ip_test(I)]
    #[test_matrix(
         [
             PhantomData::<Udp>,
             PhantomData::<Tcp>,
             PhantomData::<IcmpEchoRequest>,
         ],
         [
             PacketType::FullyParsed,
             PacketType::Raw
         ]
     )]
    fn no_icmp_error_for_normal_ip_packet<I: TestIpExt, P: Protocol>(
        _proto: PhantomData<P>,
        packet_type: PacketType,
    ) where
        for<'a> I::Packet<&'a mut [u8]>: IpPacket<I>,
        for<'a> I::PacketRaw<&'a mut [u8]>: IpPacket<I>,
    {
        let mut buf = ip_packet::<I, P>(I::SRC_IP, I::DST_IP).into_inner();
        let icmp_error = match packet_type {
            PacketType::FullyParsed => {
                let packet = I::Packet::parse_mut(SliceBufViewMut::new(&mut buf), ())
                    .expect("parse IP packet");
                let icmp_payload = packet.maybe_icmp_error().icmp_error_payload();

                icmp_payload
            }
            PacketType::Raw => {
                let packet = I::PacketRaw::parse_mut(SliceBufViewMut::new(&mut buf), ())
                    .expect("parse IP packet");
                let icmp_payload = packet.maybe_icmp_error().icmp_error_payload();

                icmp_payload
            }
        };

        assert_matches!(icmp_error, None);
    }

    #[ip_test(I)]
    #[test_matrix(
         [
             PhantomData::<Udp>,
             PhantomData::<Tcp>,
             PhantomData::<IcmpEchoRequest>,
         ],
         [
             PacketType::FullyParsed,
             PacketType::Raw
         ]
     )]
    fn no_icmp_error_mut_for_normal_ip_packet<I: TestIpExt, P: Protocol>(
        _proto: PhantomData<P>,
        packet_type: PacketType,
    ) where
        for<'a> I::Packet<&'a mut [u8]>: IpPacket<I>,
        for<'a> I::PacketRaw<&'a mut [u8]>: IpPacket<I>,
    {
        let mut buf = ip_packet::<I, P>(I::SRC_IP, I::DST_IP).into_inner();
        match packet_type {
            PacketType::FullyParsed => {
                let mut packet = I::Packet::parse_mut(SliceBufViewMut::new(&mut buf), ())
                    .expect("parse IP packet");
                assert!(packet.icmp_error_mut().icmp_error_mut().is_none());
            }
            PacketType::Raw => {
                let mut packet = I::PacketRaw::parse_mut(SliceBufViewMut::new(&mut buf), ())
                    .expect("parse IP packet");
                assert!(packet.icmp_error_mut().icmp_error_mut().is_none());
            }
        }
    }

    #[ip_test(I)]
    #[test_case(TransportPacketDataProtocol::Udp)]
    #[test_case(TransportPacketDataProtocol::Tcp)]
    #[test_case(TransportPacketDataProtocol::IcmpEchoRequest)]
    fn no_icmp_error_for_normal_bytes<I: TestIpExt>(proto: TransportPacketDataProtocol) {
        let buf = proto.make_packet::<I>(I::SRC_IP, I::DST_IP);

        assert_matches!(
            ParsedIcmpErrorPayload::<I>::parse_in_outer_ip_packet(
                proto.proto::<I>(),
                buf.as_slice(),
            ),
            None
        );
    }

    #[ip_test(I)]
    #[test_case(TransportPacketDataProtocol::Udp)]
    #[test_case(TransportPacketDataProtocol::Tcp)]
    #[test_case(TransportPacketDataProtocol::IcmpEchoRequest)]
    fn no_icmp_error_mut_for_normal_bytes<I: TestIpExt>(proto: TransportPacketDataProtocol) {
        let mut buf = proto.make_packet::<I>(I::SRC_IP, I::DST_IP);

        assert!(
            ParsedIcmpErrorMut::<I>::parse_in_ip_packet(
                I::SRC_IP,
                I::DST_IP,
                proto.proto::<I>(),
                SliceBufViewMut::new(&mut buf),
            )
            .is_none()
        );
    }

    #[ip_test(I)]
    #[test_case(PhantomData::<Udp>)]
    #[test_case(PhantomData::<Tcp>)]
    #[test_case(PhantomData::<IcmpEchoRequest>)]
    fn no_icmp_error_for_normal_serializer<I: TestIpExt, P: Protocol>(_proto: PhantomData<P>) {
        let serializer =
            P::make_serializer_with_ports::<I>(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT);

        assert_matches!(serializer.icmp_error_payload(), None);
    }

    #[ip_test(I)]
    #[test_case(PhantomData::<Udp>)]
    #[test_case(PhantomData::<Tcp>)]
    #[test_case(PhantomData::<IcmpEchoRequest>)]
    fn no_icmp_error_mut_for_normal_serializer<I: TestIpExt, P: Protocol>(_proto: PhantomData<P>) {
        let mut serializer =
            P::make_serializer_with_ports::<I>(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT);

        assert!(serializer.icmp_error_mut().is_none());
    }

    #[test_matrix(
        [
            PhantomData::<Icmpv4DestUnreachableError>,
            PhantomData::<Icmpv6DestUnreachableError>,
        ],
        [
            TransportPacketDataProtocol::Udp,
            TransportPacketDataProtocol::Tcp,
            TransportPacketDataProtocol::IcmpEchoRequest,
        ],
        [
            PacketType::FullyParsed,
            PacketType::Raw,
        ],
        [
            false,
            true,
        ]
    )]
    fn icmp_error_from_bytes<I: TestIpExt, IE: IcmpErrorMessage<I>>(
        _icmp_error: PhantomData<IE>,
        proto: TransportPacketDataProtocol,
        packet_type: PacketType,
        truncate_message: bool,
    ) {
        let serializer = IE::make_serializer_truncated(
            I::DST_IP_2,
            I::SRC_IP,
            proto.make_ip_packet_with_ports_data::<I>(
                I::SRC_IP,
                I::DST_IP,
                SRC_PORT,
                DST_PORT,
                &[0xAB; 5000],
            ),
            // Try with a truncated and full body to make sure we don't fail
            // when a partial payload is present. In these cases, the ICMP error
            // payload checksum can't be validated, though we want to be sure
            // it's updated as if it were correct.
            truncate_message.then_some(1280),
        )
        .wrap_in(I::PacketBuilder::new(I::DST_IP_2, I::SRC_IP, u8::MAX, IE::proto()));

        let mut bytes: Buf<Vec<u8>> = serializer
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b();
        let icmp_payload = match packet_type {
            PacketType::FullyParsed => {
                let packet = I::as_filter_packet_owned(bytes.parse_mut::<I::Packet<_>>().unwrap());
                let icmp_payload =
                    packet.maybe_icmp_error().icmp_error_payload().expect("no ICMP error found");

                icmp_payload
            }
            PacketType::Raw => {
                let packet =
                    I::as_filter_packet_raw_owned(bytes.parse_mut::<I::PacketRaw<_>>().unwrap());
                let icmp_payload =
                    packet.maybe_icmp_error().icmp_error_payload().expect("no ICMP error found");

                icmp_payload
            }
        };

        let expected = match proto {
            TransportPacketDataProtocol::Tcp | TransportPacketDataProtocol::Udp => {
                ParsedIcmpErrorPayload {
                    src_ip: I::SRC_IP,
                    dst_ip: I::DST_IP,
                    src_port: SRC_PORT.get(),
                    dst_port: DST_PORT.get(),
                    proto: proto.proto::<I>(),
                }
            }
            TransportPacketDataProtocol::IcmpEchoRequest => {
                ParsedIcmpErrorPayload {
                    src_ip: I::SRC_IP,
                    dst_ip: I::DST_IP,
                    // NOTE: These are intentionally the same because of how
                    // ICMP tracking works.
                    src_port: SRC_PORT.get(),
                    dst_port: SRC_PORT.get(),
                    proto: proto.proto::<I>(),
                }
            }
        };

        assert_eq!(icmp_payload, expected);
    }

    #[test_matrix(
        [
            PhantomData::<Icmpv4DestUnreachableError>,
            PhantomData::<Icmpv6DestUnreachableError>,
        ],
        [
            TransportPacketDataProtocol::Udp,
            TransportPacketDataProtocol::Tcp,
            TransportPacketDataProtocol::IcmpEchoRequest,
        ],
        [
            false,
            true,
        ]
    )]
    fn icmp_error_from_serializer<I: TestIpExt, IE: IcmpErrorMessage<I>>(
        _icmp_error: PhantomData<IE>,
        proto: TransportPacketDataProtocol,
        truncate_message: bool,
    ) {
        let serializer = IE::make_serializer_truncated(
            I::DST_IP_2,
            I::SRC_IP,
            proto.make_ip_packet_with_ports_data::<I>(
                I::SRC_IP,
                I::DST_IP,
                SRC_PORT,
                DST_PORT,
                &[0xAB; 5000],
            ),
            // Try with a truncated and full body to make sure we don't fail
            // when a partial payload is present. In these cases, the ICMP error
            // payload checksum can't be validated, though we want to be sure
            // it's updated as if it were correct.
            truncate_message.then_some(1280),
        );

        let actual =
            serializer.icmp_error_payload().expect("serializer should contain an IP packet");

        let expected = match proto {
            TransportPacketDataProtocol::Tcp | TransportPacketDataProtocol::Udp => {
                ParsedIcmpErrorPayload::<I> {
                    src_ip: I::SRC_IP,
                    dst_ip: I::DST_IP,
                    src_port: SRC_PORT.get(),
                    dst_port: DST_PORT.get(),
                    proto: proto.proto::<I>(),
                }
            }
            TransportPacketDataProtocol::IcmpEchoRequest => ParsedIcmpErrorPayload::<I> {
                src_ip: I::SRC_IP,
                dst_ip: I::DST_IP,
                // NOTE: These are intentionally the same because of how ICMP
                // tracking works.
                src_port: SRC_PORT.get(),
                dst_port: SRC_PORT.get(),
                proto: proto.proto::<I>(),
            },
        };

        assert_eq!(actual, expected);
    }

    #[test_matrix(
        [
            PhantomData::<Icmpv4DestUnreachableError>,
            PhantomData::<Icmpv6DestUnreachableError>,
        ],
        [
            TransportPacketDataProtocol::Udp,
            TransportPacketDataProtocol::Tcp,
            TransportPacketDataProtocol::IcmpEchoRequest,
        ],
        [
            PacketType::FullyParsed,
            PacketType::Raw,
        ],
        [
            false,
            true,
        ]
    )]
    fn conntrack_packet_icmp_error_from_bytes<I: TestIpExt, IE: IcmpErrorMessage<I>>(
        _icmp_error: PhantomData<IE>,
        proto: TransportPacketDataProtocol,
        packet_type: PacketType,
        truncate_message: bool,
    ) {
        let serializer = IE::make_serializer_truncated(
            I::DST_IP_2,
            I::SRC_IP,
            proto.make_ip_packet_with_ports_data::<I>(
                I::SRC_IP,
                I::DST_IP,
                SRC_PORT,
                DST_PORT,
                &[0xAB; 5000],
            ),
            // Try with a truncated and full body to make sure we don't fail
            // when a partial payload is present. In these cases, the ICMP error
            // payload checksum can't be validated, though we want to be sure
            // it's updated as if it were correct.
            truncate_message.then_some(1280),
        )
        .wrap_in(I::PacketBuilder::new(I::DST_IP_2, I::SRC_IP, u8::MAX, IE::proto()));

        let mut bytes: Buf<Vec<u8>> = serializer
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b();

        let conntrack_packet = match packet_type {
            PacketType::FullyParsed => {
                let packet = I::as_filter_packet_owned(bytes.parse_mut::<I::Packet<_>>().unwrap());
                packet.conntrack_packet().unwrap()
            }
            PacketType::Raw => {
                let packet =
                    I::as_filter_packet_raw_owned(bytes.parse_mut::<I::PacketRaw<_>>().unwrap());
                packet.conntrack_packet().unwrap()
            }
        };

        let expected = match proto {
            TransportPacketDataProtocol::Tcp | TransportPacketDataProtocol::Udp => {
                conntrack::PacketMetadata::new_from_icmp_error(
                    I::SRC_IP,
                    I::DST_IP,
                    SRC_PORT.get(),
                    DST_PORT.get(),
                    I::map_ip(proto.proto::<I>(), |proto| proto.into(), |proto| proto.into()),
                )
            }
            TransportPacketDataProtocol::IcmpEchoRequest => {
                conntrack::PacketMetadata::new_from_icmp_error(
                    I::SRC_IP,
                    I::DST_IP,
                    // NOTE: These are intentionally the same because of how
                    // ICMP tracking works.
                    SRC_PORT.get(),
                    SRC_PORT.get(),
                    I::map_ip(proto.proto::<I>(), |proto| proto.into(), |proto| proto.into()),
                )
            }
        };

        assert_eq!(conntrack_packet, expected);
    }

    #[test_matrix(
        [
            PhantomData::<Icmpv4DestUnreachableError>,
            PhantomData::<Icmpv6DestUnreachableError>,
        ],
        [
            TransportPacketDataProtocol::Udp,
            TransportPacketDataProtocol::Tcp,
            TransportPacketDataProtocol::IcmpEchoRequest,
        ],
        [
            PacketType::FullyParsed,
            PacketType::Raw,
        ],
        [
            false,
            true,
        ]
    )]
    fn no_conntrack_packet_for_incompatible_outer_and_payload<
        I: TestIpExt,
        IE: IcmpErrorMessage<I>,
    >(
        _icmp_error: PhantomData<IE>,
        proto: TransportPacketDataProtocol,
        packet_type: PacketType,
        truncate_message: bool,
    ) {
        // In order for the outer packet to have the tuple (DST_IP_2, SRC_IP_2),
        // the host sending the error must have seen a packet with a source
        // address of SRC_IP_2, but we know that can't be right because the
        // payload of the packet contains a packet with a source address of
        // SRC_IP.
        let serializer = IE::make_serializer_truncated(
            I::DST_IP_2,
            I::SRC_IP_2,
            proto.make_ip_packet_with_ports_data::<I>(
                I::SRC_IP,
                I::DST_IP,
                SRC_PORT,
                DST_PORT,
                &[0xAB; 5000],
            ),
            // Try with a truncated and full body to make sure we don't fail
            // when a partial payload is present. In these cases, the ICMP error
            // payload checksum can't be validated, though we want to be sure
            // it's updated as if it were correct.
            truncate_message.then_some(1280),
        )
        .wrap_in(I::PacketBuilder::new(I::DST_IP_2, I::SRC_IP_2, u8::MAX, IE::proto()));

        let mut bytes: Buf<Vec<u8>> = serializer
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b();

        let conntrack_packet = match packet_type {
            PacketType::FullyParsed => {
                let packet = I::as_filter_packet_owned(bytes.parse_mut::<I::Packet<_>>().unwrap());
                packet.conntrack_packet()
            }
            PacketType::Raw => {
                let packet =
                    I::as_filter_packet_raw_owned(bytes.parse_mut::<I::PacketRaw<_>>().unwrap());
                packet.conntrack_packet()
            }
        };

        // Because the outer and payload tuples aren't compatible, we shouldn't
        // get a conntrack packet back.
        assert_matches!(conntrack_packet, None);
    }

    #[test_matrix(
        [
            PhantomData::<Icmpv4DestUnreachableError>,
            PhantomData::<Icmpv6DestUnreachableError>,
        ],
        [
            TransportPacketDataProtocol::Udp,
            TransportPacketDataProtocol::Tcp,
            TransportPacketDataProtocol::IcmpEchoRequest,
        ],
        [
            false,
            true,
        ]
    )]
    fn icmp_error_mut_from_serializer<I: TestIpExt, IE: IcmpErrorMessage<I>>(
        _icmp_error: PhantomData<IE>,
        proto: TransportPacketDataProtocol,
        truncate_message: bool,
    ) where
        for<'a> I::Packet<&'a mut [u8]>: IpPacket<I>,
    {
        const LEN: usize = 5000;

        let mut payload_bytes = proto.make_ip_packet_with_ports_data::<I>(
            I::SRC_IP,
            I::DST_IP,
            SRC_PORT,
            DST_PORT,
            &[0xAB; LEN],
        );

        // Try with a truncated and full body to make sure we don't fail when a
        // partial payload is present.
        if truncate_message {
            payload_bytes.truncate(1280);
        }

        let mut serializer = IE::make_serializer(I::SRC_IP, I::DST_IP, payload_bytes)
            .wrap_in(I::PacketBuilder::new(I::SRC_IP, I::DST_IP, u8::MAX, IE::proto()));

        {
            let mut icmp_packet = serializer
                .icmp_error_mut()
                .icmp_error_mut()
                .expect("couldn't find an inner ICMP error");

            {
                let mut inner_packet = icmp_packet.inner_packet().expect("no inner packet");

                inner_packet.set_src_addr(I::SRC_IP_2);
                inner_packet.set_dst_addr(I::DST_IP_2);
            }

            // Since this is just a serializer, there's no thing to be recalculated,
            // but this should still never fail.
            assert!(icmp_packet.recalculate_checksum());
        }

        let mut expected_payload_bytes = proto.make_ip_packet_with_ports_data::<I>(
            I::SRC_IP_2,
            I::DST_IP_2,
            SRC_PORT,
            DST_PORT,
            &[0xAB; LEN],
        );

        // Try with a truncated and full body to make sure we don't fail when a
        // partial payload is present.
        if truncate_message {
            expected_payload_bytes.truncate(1280);
        }

        let expected_serializer = IE::make_serializer(I::SRC_IP, I::DST_IP, expected_payload_bytes)
            // We never updated the outer IPs, so they should still be
            // their original values.
            .wrap_in(I::PacketBuilder::new(I::SRC_IP, I::DST_IP, u8::MAX, IE::proto()));

        let actual_bytes = serializer
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b();
        let expected_bytes = expected_serializer
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b();

        assert_eq!(actual_bytes, expected_bytes);
    }

    #[test_matrix(
        [
            PhantomData::<Icmpv4DestUnreachableError>,
            PhantomData::<Icmpv6DestUnreachableError>,
        ],
        [
            TransportPacketDataProtocol::Udp,
            TransportPacketDataProtocol::Tcp,
            TransportPacketDataProtocol::IcmpEchoRequest,
        ],
        [
            PacketType::FullyParsed,
            PacketType::Raw,
        ],
        [
            false,
            true,
        ]
    )]
    fn icmp_error_mut_from_bytes<I: TestIpExt, IE: IcmpErrorMessage<I>>(
        _icmp_error: PhantomData<IE>,
        proto: TransportPacketDataProtocol,
        packet_type: PacketType,
        truncate_message: bool,
    ) where
        for<'a> I::Packet<&'a mut [u8]>: IpPacket<I>,
    {
        const LEN: usize = 5000;

        let mut payload_bytes = proto.make_ip_packet_with_ports_data::<I>(
            I::SRC_IP,
            I::DST_IP,
            SRC_PORT,
            DST_PORT,
            &[0xAB; LEN],
        );

        // Try with a truncated and full body to make sure we don't fail when a
        // partial payload is present.
        if truncate_message {
            payload_bytes.truncate(1280);
        }

        let serializer = IE::make_serializer(I::SRC_IP, I::DST_IP, payload_bytes)
            .wrap_in(I::PacketBuilder::new(I::SRC_IP, I::DST_IP, u8::MAX, IE::proto()));

        let mut bytes = serializer
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b()
            .into_inner();

        {
            fn modify_packet<I: TestIpExt, P: IpPacket<I>>(mut packet: P) {
                let mut icmp_error = packet.icmp_error_mut();
                let mut icmp_error =
                    icmp_error.icmp_error_mut().expect("couldn't find an inner ICMP error");

                {
                    let mut inner_packet = icmp_error.inner_packet().expect("no inner packet");

                    inner_packet.set_src_addr(I::SRC_IP_2);
                    inner_packet.set_dst_addr(I::DST_IP_2);
                }

                assert!(icmp_error.recalculate_checksum());
            }

            let mut bytes = Buf::new(&mut bytes, ..);

            match packet_type {
                PacketType::FullyParsed => {
                    let packet =
                        I::as_filter_packet_owned(bytes.parse_mut::<I::Packet<_>>().unwrap());
                    modify_packet(packet);
                }
                PacketType::Raw => {
                    let packet = I::as_filter_packet_raw_owned(
                        bytes.parse_mut::<I::PacketRaw<_>>().unwrap(),
                    );
                    modify_packet(packet);
                }
            }
        }

        let mut expected_payload_bytes = proto.make_ip_packet_with_ports_data::<I>(
            I::SRC_IP_2,
            I::DST_IP_2,
            SRC_PORT,
            DST_PORT,
            &[0xAB; LEN],
        );

        if truncate_message {
            expected_payload_bytes.truncate(1280);
        }

        let expected_serializer = IE::make_serializer(I::SRC_IP, I::DST_IP, expected_payload_bytes)
            // We never updated the outer IPs, so they should still be
            // their original values.
            .wrap_in(I::PacketBuilder::new(I::SRC_IP, I::DST_IP, u8::MAX, IE::proto()));

        let expected_bytes = expected_serializer
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b()
            .into_inner();

        assert_eq!(bytes, expected_bytes);
    }

    #[ip_test(I)]
    #[test_case(PhantomData::<Udp>)]
    #[test_case(PhantomData::<Tcp>)]
    #[test_case(PhantomData::<IcmpEchoRequest>)]
    fn tx_packet_partial_serialize<I: TestIpExt, P: Protocol>(_proto: PhantomData<P>) {
        const DATA: &[u8] = b"Packet Body";
        let mut body =
            P::make_serializer_with_ports_data::<I>(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT, DATA);
        let packet = TxPacket::<I, _>::new(I::SRC_IP, I::DST_IP, P::proto::<I>(), &mut body);

        let (buf, total_size) = PartialSerializer::partial_serialize_new_buf(
            &packet,
            &mut NetworkSerializationContext::default(),
            PacketConstraints::UNCONSTRAINED,
            packet::new_buf_vec,
        )
        .unwrap();

        let whole_packet =
            P::make_serializer_with_ports_data::<I>(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT, DATA)
                .wrap_in(I::PacketBuilder::new(
                    I::SRC_IP,
                    I::DST_IP,
                    TX_PACKET_NO_TTL,
                    P::proto::<I>(),
                ))
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .expect("serialize packet")
                .unwrap_b()
                .into_inner();

        let headers_size = I::MIN_HEADER_LENGTH + P::HEADER_SIZE;
        assert_eq!(total_size, whole_packet.len());
        assert_eq!(buf.len(), headers_size);

        // Count the number of bytes that are different in the partially
        // serialized packet headers.
        let num_bytes_differ = buf
            .as_ref()
            .iter()
            .zip(whole_packet[..headers_size].iter())
            .map(|(a, b)| if a != b { 1 } else { 0 })
            .sum::<usize>();

        // Partial serializer doesn't calculate packet checksum. IPv6 header
        // doesn't contain a checksum, but IPv4 header and transport layer
        // headers contain 2 bytes for checksum each. Only these bytes may
        // differ from a fully-serialized packet.
        let checksum_bytes = I::map_ip((), |()| 4, |()| 2);
        assert!(num_bytes_differ <= checksum_bytes);
    }

    #[ip_test(I)]
    #[test_case(PhantomData::<Udp>)]
    #[test_case(PhantomData::<Tcp>)]
    #[test_case(PhantomData::<IcmpEchoRequest>)]
    fn tx_packet_raw_ip_body_partial_serialize<I: TestIpExt, P: Protocol>(_proto: PhantomData<P>) {
        const DATA: &[u8] = b"Packet Body";
        let body_bytes =
            P::make_serializer_with_ports_data::<I>(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT, DATA)
                .serialize_vec_outer(&mut NetworkSerializationContext::default())
                .unwrap()
                .unwrap_b()
                .into_inner();
        let body_bytes_len = body_bytes.len();
        let mut body =
            RawIpBody::new(P::proto::<I>(), I::SRC_IP, I::DST_IP, Buf::new(body_bytes.clone(), ..));
        let packet = TxPacket::<I, _>::new(I::SRC_IP, I::DST_IP, P::proto::<I>(), &mut body);

        let (buf, total_size) = PartialSerializer::partial_serialize_new_buf(
            &packet,
            &mut NetworkSerializationContext::default(),
            PacketConstraints::UNCONSTRAINED,
            packet::new_buf_vec,
        )
        .unwrap();

        let whole_packet = Buf::new(body_bytes, ..)
            .wrap_in(I::PacketBuilder::new(I::SRC_IP, I::DST_IP, TX_PACKET_NO_TTL, P::proto::<I>()))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .expect("serialize packet")
            .unwrap_b()
            .into_inner();

        let headers_size =
            I::MIN_HEADER_LENGTH + cmp::min(body_bytes_len, TRANSPORT_HEADER_MAX_SIZE);
        assert_eq!(total_size, whole_packet.len());
        assert_eq!(buf.len(), headers_size);

        // Count the number of bytes that are different in the partially
        // serialized packet headers.
        let num_bytes_differ = buf
            .as_ref()
            .iter()
            .zip(whole_packet[..headers_size].iter())
            .map(|(a, b)| if a != b { 1 } else { 0 })
            .sum::<usize>();

        // Partial serializer doesn't calculate packet checksum. IPv6 header
        // doesn't contain a checksum, but IPv4 header contains 2 bytes for checksum.
        // The transport header checksum is already calculated because we fully
        // serialized it to put in RawIpBody.
        let checksum_bytes = I::map_ip((), |()| 2, |()| 0);
        assert!(num_bytes_differ <= checksum_bytes);
    }

    #[ip_test(I)]
    #[test_case(PhantomData::<Udp>)]
    #[test_case(PhantomData::<Tcp>)]
    #[test_case(PhantomData::<IcmpEchoRequest>)]
    fn forwarded_packet_partial_serialize<I: TestIpExt, P: Protocol>(_proto: PhantomData<P>) {
        let mut packet_buf = ip_packet::<I, P>(I::SRC_IP, I::DST_IP);
        let packet_bytes = packet_buf.to_flattened_vec();
        let meta = packet_buf.parse::<I::Packet<_>>().expect("parse IP packet").parse_metadata();
        let packet = ForwardedPacket::<I, _>::new(
            I::SRC_IP,
            I::DST_IP,
            P::proto::<I>(),
            meta,
            packet_buf,
            false,
        );

        let result = packet
            .partial_serialize(&mut NetworkSerializationContext::default(), packet::new_buf_vec)
            .unwrap();
        assert_eq!(result, PartialSerializeResult::Slice(&packet_bytes[..]));

        let (buf, total_size) = PartialSerializer::partial_serialize_new_buf(
            &packet,
            &mut NetworkSerializationContext::default(),
            PacketConstraints::UNCONSTRAINED,
            packet::new_buf_vec,
        )
        .unwrap();

        let expected_len =
            cmp::min(packet_bytes.len(), meta.header_len() + TRANSPORT_HEADER_MAX_SIZE);
        assert_eq!(total_size, packet_bytes.len());
        assert_eq!(buf.as_ref(), &packet_bytes[..expected_len]);
    }
}
