// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! ICMPv6

use core::fmt;

use net_types::ip::{GenericOverIp, Ipv6, Ipv6Addr};
use packet::{BufferView, ParsablePacket, ParseMetadata};
use zerocopy::byteorder::network_endian::U32;
use zerocopy::{
    FromBytes, Immutable, IntoBytes, KnownLayout, SplitByteSlice, SplitByteSliceMut, Unaligned,
};

use crate::error::{ParseError, ParseResult};

use super::common::{IcmpDestUnreachable, IcmpEchoReply, IcmpEchoRequest, IcmpTimeExceeded};
use super::{
    HeaderPrefix, IcmpIpExt, IcmpMessageType, IcmpPacket, IcmpPacketRaw, IcmpParseArgs,
    IcmpZeroCode, OriginalPacket, mld, ndp, peek_message_type,
};

/// Dispatches expressions to the type-safe variants of [`Icmpv6Packet`] or
/// [`Icmpv6PacketRaw`].
///
/// # Usage
///
/// ```
/// icmpv6_dispatch!(packet, p => p.message_mut().foo());
///
/// icmpv6_dispatch!(packet: raw, p => p.message_mut().foo());
/// ```
#[macro_export]
macro_rules! icmpv6_dispatch {
    (__internal__, $x:ident, $variable:pat => $expression:expr, $($typ:ty),+) => {
        {
            $(
                use $typ::*;
            )+

            match $x {
                DestUnreachable($variable) => $expression,
                PacketTooBig($variable) => $expression,
                TimeExceeded($variable) => $expression,
                ParameterProblem($variable) => $expression,
                EchoRequest($variable) => $expression,
                EchoReply($variable) => $expression,
                Ndp(RouterSolicitation($variable)) => $expression,
                Ndp(RouterAdvertisement($variable)) => $expression,
                Ndp(NeighborSolicitation($variable)) => $expression,
                Ndp(NeighborAdvertisement($variable)) => $expression,
                Ndp(Redirect($variable)) => $expression,
                Mld(MulticastListenerQuery($variable)) => $expression,
                Mld(MulticastListenerReport($variable)) => $expression,
                Mld(MulticastListenerDone($variable)) => $expression,
                Mld(MulticastListenerQueryV2($variable)) => $expression,
                Mld(MulticastListenerReportV2($variable)) => $expression,
            }
        }
    };
    ($x:ident : raw, $variable:pat => $expression:expr) => {
        $crate::icmpv6_dispatch!(__internal__, $x,
            $variable => $expression,
            $crate::icmp::Icmpv6PacketRaw,
            $crate::icmp::mld::MldPacketRaw,
            $crate::icmp::ndp::NdpPacketRaw)
    };
    ($x:ident, $variable:pat => $expression:expr) => {
        $crate::icmpv6_dispatch!(__internal__, $x,
            $variable => $expression,
            $crate::icmp::Icmpv6Packet,
            $crate::icmp::mld::MldPacket,
            $crate::icmp::ndp::NdpPacket)
    };
}

/// An ICMPv6 packet with a dynamic message type.
///
/// Unlike `IcmpPacket`, `Packet` only supports ICMPv6, and does not
/// require a static message type. Each enum variant contains an `IcmpPacket` of
/// the appropriate static type, making it easier to call `parse` without
/// knowing the message type ahead of time while still getting the benefits of a
/// statically-typed packet struct after parsing is complete.
#[allow(missing_docs)]
pub enum Icmpv6Packet<B: SplitByteSlice> {
    DestUnreachable(IcmpPacket<Ipv6, B, IcmpDestUnreachable>),
    PacketTooBig(IcmpPacket<Ipv6, B, Icmpv6PacketTooBig>),
    TimeExceeded(IcmpPacket<Ipv6, B, IcmpTimeExceeded>),
    ParameterProblem(IcmpPacket<Ipv6, B, Icmpv6ParameterProblem>),
    EchoRequest(IcmpPacket<Ipv6, B, IcmpEchoRequest>),
    EchoReply(IcmpPacket<Ipv6, B, IcmpEchoReply>),
    Ndp(ndp::NdpPacket<B>),
    Mld(mld::MldPacket<B>),
}

impl<B: SplitByteSlice + fmt::Debug> fmt::Debug for Icmpv6Packet<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use self::Icmpv6Packet::*;
        use mld::MldPacket::*;
        use ndp::NdpPacket::*;
        match self {
            DestUnreachable(p) => f.debug_tuple("DestUnreachable").field(p).finish(),
            PacketTooBig(p) => f.debug_tuple("PacketTooBig").field(p).finish(),
            TimeExceeded(p) => f.debug_tuple("TimeExceeded").field(p).finish(),
            ParameterProblem(p) => f.debug_tuple("ParameterProblem").field(p).finish(),
            EchoRequest(p) => f.debug_tuple("EchoRequest").field(p).finish(),
            EchoReply(p) => f.debug_tuple("EchoReply").field(p).finish(),
            Ndp(RouterSolicitation(p)) => f.debug_tuple("RouterSolicitation").field(p).finish(),
            Ndp(RouterAdvertisement(p)) => f.debug_tuple("RouterAdvertisement").field(p).finish(),
            Ndp(NeighborSolicitation(p)) => f.debug_tuple("NeighborSolicitation").field(p).finish(),
            Ndp(NeighborAdvertisement(p)) => {
                f.debug_tuple("NeighborAdvertisement").field(p).finish()
            }
            Ndp(Redirect(p)) => f.debug_tuple("Redirect").field(p).finish(),
            Mld(MulticastListenerQuery(p)) => {
                f.debug_tuple("MulticastListenerQuery").field(p).finish()
            }
            Mld(MulticastListenerReport(p)) => {
                f.debug_tuple("MulticastListenerReport").field(p).finish()
            }
            Mld(MulticastListenerDone(p)) => {
                f.debug_tuple("MulticastListenerDone").field(p).finish()
            }
            Mld(MulticastListenerQueryV2(p)) => {
                f.debug_tuple("MulticastListenerQueryV2").field(p).finish()
            }
            Mld(MulticastListenerReportV2(p)) => {
                f.debug_tuple("MulticastListenerReportV2").field(p).finish()
            }
        }
    }
}

impl<B: SplitByteSlice> ParsablePacket<B, IcmpParseArgs<Ipv6Addr>> for Icmpv6Packet<B> {
    type Error = ParseError;

    fn parse_metadata(&self) -> ParseMetadata {
        icmpv6_dispatch!(self, p => p.parse_metadata())
    }

    fn parse<BV: BufferView<B>>(buffer: BV, args: IcmpParseArgs<Ipv6Addr>) -> ParseResult<Self> {
        use self::Icmpv6Packet::*;
        use mld::MldPacket::*;
        use ndp::NdpPacket::*;

        macro_rules! mtch {
            ($buffer:expr, $args:expr, $pkt_name:ident, $( ($msg_variant:ident, $len_pat:pat) => $pkt_variant:expr => $type:ty,)*) => {
                match ( peek_message_type($buffer.as_ref())?, $buffer.len() ) {
                    $( (Icmpv6MessageType::$msg_variant, $len_pat) => {
                        let $pkt_name = <IcmpPacket<Ipv6, B, $type> as ParsablePacket<_, _>>::parse($buffer, $args)?;
                        $pkt_variant
                    })*
                }
            }
        }

        Ok(mtch!(
            buffer,
            args,
            packet,
            (DestUnreachable, ..)               => DestUnreachable(packet)                => IcmpDestUnreachable,
            (PacketTooBig, ..)                  => PacketTooBig(packet)                   => Icmpv6PacketTooBig,
            (TimeExceeded, ..)                  => TimeExceeded(packet)                   => IcmpTimeExceeded,
            (ParameterProblem, ..)              => ParameterProblem(packet)               => Icmpv6ParameterProblem,
            (EchoRequest, ..)                   => EchoRequest(packet)                    => IcmpEchoRequest,
            (EchoReply, ..)                     => EchoReply(packet)                      => IcmpEchoReply,
            (RouterSolicitation, ..)            => Ndp(RouterSolicitation(packet))        => ndp::RouterSolicitation,
            (RouterAdvertisement, ..)           => Ndp(RouterAdvertisement(packet))       => ndp::RouterAdvertisement,
            (NeighborSolicitation, ..)          => Ndp(NeighborSolicitation(packet))      => ndp::NeighborSolicitation,
            (NeighborAdvertisement, ..)         => Ndp(NeighborAdvertisement(packet))     => ndp::NeighborAdvertisement,
            (Redirect, ..)                      => Ndp(Redirect(packet))                  => ndp::Redirect,
            (MulticastListenerQuery, 0..=27 )   => Mld(MulticastListenerQuery(packet))    => mld::MulticastListenerQuery,
            (MulticastListenerQuery, 28..)      => Mld(MulticastListenerQueryV2(packet))  => mld::MulticastListenerQueryV2,
            (MulticastListenerReport, ..)       => Mld(MulticastListenerReport(packet))   => mld::MulticastListenerReport,
            (MulticastListenerReportV2, ..)     => Mld(MulticastListenerReportV2(packet)) => mld::MulticastListenerReportV2,
            (MulticastListenerDone, ..)         => Mld(MulticastListenerDone(packet))     => mld::MulticastListenerDone,
        ))
    }
}

/// A raw ICMPv6 packet with a dynamic message type.
///
/// Unlike `IcmpPacketRaw`, `Packet` only supports ICMPv6, and does not
/// require a static message type. Each enum variant contains an `IcmpPacketRaw`
/// of the appropriate static type, making it easier to call `parse` without
/// knowing the message type ahead of time while still getting the benefits of a
/// statically-typed packet struct after parsing is complete.
#[allow(missing_docs)]
pub enum Icmpv6PacketRaw<B: SplitByteSlice> {
    DestUnreachable(IcmpPacketRaw<Ipv6, B, IcmpDestUnreachable>),
    PacketTooBig(IcmpPacketRaw<Ipv6, B, Icmpv6PacketTooBig>),
    TimeExceeded(IcmpPacketRaw<Ipv6, B, IcmpTimeExceeded>),
    ParameterProblem(IcmpPacketRaw<Ipv6, B, Icmpv6ParameterProblem>),
    EchoRequest(IcmpPacketRaw<Ipv6, B, IcmpEchoRequest>),
    EchoReply(IcmpPacketRaw<Ipv6, B, IcmpEchoReply>),
    Ndp(ndp::NdpPacketRaw<B>),
    Mld(mld::MldPacketRaw<B>),
}

impl<B: SplitByteSliceMut> Icmpv6PacketRaw<B> {
    pub(super) fn header_prefix_mut(&mut self) -> &mut HeaderPrefix {
        icmpv6_dispatch!(self: raw, p => &mut p.header.prefix)
    }

    /// Overwrites the current checksum with `checksum`, returning the original.
    pub(crate) fn overwrite_checksum(&mut self, checksum: [u8; 2]) -> [u8; 2] {
        core::mem::replace(&mut self.header_prefix_mut().checksum, checksum)
    }

    /// Attempts to calculate and write a Checksum for this [`Icmpv6PacketRaw`].
    ///
    /// Returns whether the checksum was successfully calculated & written. In
    /// the false case, self is left unmodified.
    pub fn try_write_checksum(&mut self, src_ip: Ipv6Addr, dst_ip: Ipv6Addr) -> bool {
        icmpv6_dispatch!(self: raw, p => p.try_write_checksum(src_ip, dst_ip))
    }
}

impl<B: SplitByteSlice> ParsablePacket<B, ()> for Icmpv6PacketRaw<B> {
    type Error = ParseError;

    fn parse_metadata(&self) -> ParseMetadata {
        icmpv6_dispatch!(self: raw, p => p.parse_metadata())
    }

    fn parse<BV: BufferView<B>>(buffer: BV, _args: ()) -> ParseResult<Self> {
        use self::Icmpv6PacketRaw::*;
        use mld::MldPacketRaw::*;
        use ndp::NdpPacketRaw::*;

        macro_rules! mtch {
            ($buffer:expr, $pkt_name:ident, $( ($msg_variant:ident, $len_pat:pat) => $pkt_variant:expr => $type:ty,)*) => {
                match ( peek_message_type($buffer.as_ref())?, $buffer.len() ) {
                    $( (Icmpv6MessageType::$msg_variant, $len_pat) => {
                        let $pkt_name = <IcmpPacketRaw<Ipv6, B, $type> as ParsablePacket<_, _>>::parse($buffer, ())?;
                        $pkt_variant
                    })*
                }
            }
        }

        Ok(mtch!(
            buffer,
            packet,
            (DestUnreachable, ..)               => DestUnreachable(packet)                => IcmpDestUnreachable,
            (PacketTooBig, ..)                  => PacketTooBig(packet)                   => Icmpv6PacketTooBig,
            (TimeExceeded, ..)                  => TimeExceeded(packet)                   => IcmpTimeExceeded,
            (ParameterProblem, ..)              => ParameterProblem(packet)               => Icmpv6ParameterProblem,
            (EchoRequest, ..)                   => EchoRequest(packet)                    => IcmpEchoRequest,
            (EchoReply, ..)                     => EchoReply(packet)                      => IcmpEchoReply,
            (RouterSolicitation, ..)            => Ndp(RouterSolicitation(packet))        => ndp::RouterSolicitation,
            (RouterAdvertisement, ..)           => Ndp(RouterAdvertisement(packet))       => ndp::RouterAdvertisement,
            (NeighborSolicitation, ..)          => Ndp(NeighborSolicitation(packet))      => ndp::NeighborSolicitation,
            (NeighborAdvertisement, ..)         => Ndp(NeighborAdvertisement(packet))     => ndp::NeighborAdvertisement,
            (Redirect, ..)                      => Ndp(Redirect(packet))                  => ndp::Redirect,
            (MulticastListenerQuery, 0..=27 )   => Mld(MulticastListenerQuery(packet))    => mld::MulticastListenerQuery,
            (MulticastListenerQuery, 28..)      => Mld(MulticastListenerQueryV2(packet))  => mld::MulticastListenerQueryV2,
            (MulticastListenerReport, ..)       => Mld(MulticastListenerReport(packet))   => mld::MulticastListenerReport,
            (MulticastListenerReportV2, ..)     => Mld(MulticastListenerReportV2(packet)) => mld::MulticastListenerReportV2,
            (MulticastListenerDone, ..)         => Mld(MulticastListenerDone(packet))     => mld::MulticastListenerDone,
        ))
    }
}

create_protocol_enum!(
    #[allow(missing_docs)]
    #[derive(Copy, Clone, PartialEq, Eq)]
    pub enum Icmpv6MessageType: u8 {
        DestUnreachable, 1, "Destination Unreachable";
        PacketTooBig, 2, "Packet Too Big";
        TimeExceeded, 3, "Time Exceeded";
        ParameterProblem, 4, "Parameter Problem";
        EchoRequest, 128, "Echo Request";
        EchoReply, 129, "Echo Reply";

        // NDP messages
        RouterSolicitation, 133, "Router Solicitation";
        RouterAdvertisement, 134, "Router Advertisement";
        NeighborSolicitation, 135, "Neighbor Solicitation";
        NeighborAdvertisement, 136, "Neighbor Advertisement";
        Redirect, 137, "Redirect";

        // MLDv1 messages
        // This is used for both QueryV1 and QueryV2
        MulticastListenerQuery, 130, "Multicast Listener Query";
        MulticastListenerReport, 131, "Multicast Listener Report";
        MulticastListenerDone, 132, "Multicast Listener Done";

        // MLDv2 messages
        MulticastListenerReportV2, 143, "Multicast Listener Report V2";
    }
);

impl<I: IcmpIpExt> GenericOverIp<I> for Icmpv6MessageType {
    type Type = I::IcmpMessageType;
}

impl IcmpMessageType for Icmpv6MessageType {
    fn is_error_or_redirect(self) -> bool {
        match self {
            Icmpv6MessageType::DestUnreachable
            | Icmpv6MessageType::PacketTooBig
            | Icmpv6MessageType::TimeExceeded
            | Icmpv6MessageType::ParameterProblem
            | Icmpv6MessageType::Redirect => true,
            _ => false,
        }
    }
}

create_protocol_enum!(
    #[allow(missing_docs)]
    #[derive(Copy, Clone, PartialEq, Eq)]
    pub enum Icmpv6DestUnreachableCode: u8 {
        NoRoute, 0, "No Route";
        CommAdministrativelyProhibited, 1, "Comm Administratively Prohibited";
        BeyondScope, 2, "Beyond Scope";
        AddrUnreachable, 3, "Address Unreachable";
        PortUnreachable, 4, "Port Unreachable";
        SrcAddrFailedPolicy, 5, "Source Address Failed Policy";
        RejectRoute, 6, "Reject Route";
    }
);

impl_icmp_message!(
    Ipv6,
    IcmpDestUnreachable,
    DestUnreachable,
    Icmpv6DestUnreachableCode,
    OriginalPacket<B>
);

/// An ICMPv6 Packet Too Big message.
#[derive(
    Copy, Clone, Debug, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned, PartialEq,
)]
#[repr(C)]
pub struct Icmpv6PacketTooBig {
    mtu: U32,
}

impl Icmpv6PacketTooBig {
    /// Returns a new `Icmpv6PacketTooBig` with the given MTU value.
    pub fn new(mtu: u32) -> Icmpv6PacketTooBig {
        Icmpv6PacketTooBig { mtu: U32::new(mtu) }
    }

    /// Get the mtu value.
    pub fn mtu(&self) -> u32 {
        self.mtu.get()
    }
}

impl_icmp_message!(Ipv6, Icmpv6PacketTooBig, PacketTooBig, IcmpZeroCode, OriginalPacket<B>);

create_protocol_enum!(
    #[allow(missing_docs)]
    #[derive(Copy, Clone, PartialEq, Eq)]
    pub enum Icmpv6TimeExceededCode: u8 {
        HopLimitExceeded, 0, "Hop Limit Exceeded";
        FragmentReassemblyTimeExceeded, 1, "Fragment Reassembly Time Exceeded";
    }
);

impl_icmp_message!(Ipv6, IcmpTimeExceeded, TimeExceeded, Icmpv6TimeExceededCode, OriginalPacket<B>);

create_protocol_enum!(
    #[allow(missing_docs)]
    #[derive(Copy, Clone, PartialEq, Eq)]
    pub enum Icmpv6ParameterProblemCode: u8 {
        ErroneousHeaderField, 0, "Erroneous Header Field";
        UnrecognizedNextHeaderType, 1, "Unrecognized Next Header Type";
        UnrecognizedIpv6Option, 2, "Unrecognized IPv6 Option";
    }
);

/// An ICMPv6 Parameter Problem message.
#[derive(
    Copy, Clone, Debug, Eq, PartialEq, KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned,
)]
#[repr(C)]
pub struct Icmpv6ParameterProblem {
    pointer: U32,
}

impl Icmpv6ParameterProblem {
    /// Returns a new `Icmpv6ParameterProblem` with the given pointer.
    pub fn new(pointer: u32) -> Icmpv6ParameterProblem {
        Icmpv6ParameterProblem { pointer: U32::new(pointer) }
    }

    /// Gets the pointer of the ICMPv6 Parameter Problem message.
    pub fn pointer(self) -> u32 {
        self.pointer.get()
    }
}

impl_icmp_message!(
    Ipv6,
    Icmpv6ParameterProblem,
    ParameterProblem,
    Icmpv6ParameterProblemCode,
    OriginalPacket<B>
);

#[cfg(test)]
mod tests {
    use core::fmt::Debug;
    use packet::{
        InnerPacketBuilder, NestableSerializer as _, NoOpSerializationContext, ParseBuffer,
        Serializer,
    };

    use super::*;
    use crate::icmp::{IcmpMessage, MessageBody};
    use crate::ipv6::{Ipv6Header, Ipv6Packet, Ipv6PacketBuilder};

    fn serialize_to_bytes<B: SplitByteSlice + Debug, M: IcmpMessage<Ipv6> + Debug>(
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        icmp: &IcmpPacket<Ipv6, B, M>,
        builder: Ipv6PacketBuilder,
    ) -> Vec<u8> {
        let (header, body) = icmp.message_body.bytes();
        let body = if let Some(b) = body { b } else { &[] };
        let complete_msg = &[header, body].concat();

        complete_msg
            .into_serializer()
            .wrap_in(icmp.builder(src_ip, dst_ip))
            .wrap_in(builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .as_ref()
            .to_vec()
    }

    fn test_parse_and_serialize<
        M: IcmpMessage<Ipv6> + Debug,
        F: for<'a> FnOnce(&IcmpPacket<Ipv6, &'a [u8], M>),
    >(
        mut req: &[u8],
        check: F,
    ) {
        let orig_req = req;

        let ip = req.parse::<Ipv6Packet<_>>().unwrap();
        let mut body = ip.body();
        let icmp = body
            .parse_with::<_, IcmpPacket<_, _, M>>(IcmpParseArgs::new(ip.src_ip(), ip.dst_ip()))
            .unwrap();
        check(&icmp);

        let data = serialize_to_bytes(ip.src_ip(), ip.dst_ip(), &icmp, ip.builder());
        assert_eq!(&data[..], orig_req);
    }

    #[test]
    fn test_parse_and_serialize_echo_request() {
        use crate::testdata::icmp_echo_v6::*;
        test_parse_and_serialize::<IcmpEchoRequest, _>(REQUEST_IP_PACKET_BYTES, |icmp| {
            let (inner_header, inner_body) = icmp.message_body.bytes();
            assert!(inner_body.is_none());
            let complete_msg = inner_header;
            assert_eq!(complete_msg, ECHO_DATA);
            assert_eq!(icmp.message().id_seq.id.get(), IDENTIFIER);
            assert_eq!(icmp.message().id_seq.seq.get(), SEQUENCE_NUM);
        });
    }
}
