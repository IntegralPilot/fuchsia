// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Contexts for packet parsing and serialization in netstack3.

use crate::inspect::{Inspectable, Inspector};
use bitflags::bitflags;
use core::num::NonZeroU16;
use net_types::ip::IpInvariant;
use packet::{
    DynamicPartialSerializer, DynamicSerializer, PacketBuilder, PacketConstraints,
    PartialSerializer, SerializationContext, Serializer,
};
use packet_formats::TransportChecksumAction;
use packet_formats::ethernet::{EthernetEnvelope, EthernetSerializationContext};
use packet_formats::icmp::{IcmpEnvelope, IcmpSerializationContext};
use packet_formats::ip::{IpEnvelope, IpExt, IpSerializationContext};
use packet_formats::tcp::{TcpEnvelope, TcpParseContext, TcpSerializationContext};
use packet_formats::udp::{UdpEnvelope, UdpParseContext, UdpSerializationContext};
use static_assertions::const_assert;

/// The specific packet `Serializer` type used within netstack3.
pub trait NetworkSerializer: Serializer<NetworkSerializationContext> {}
impl<S: Serializer<NetworkSerializationContext>> NetworkSerializer for S {}

/// The specific packet `PartialSerializer` type used within netstack3.
pub trait NetworkPartialSerializer: PartialSerializer<NetworkSerializationContext> {}
impl<S: PartialSerializer<NetworkSerializationContext>> NetworkPartialSerializer for S {}

/// The specific dynamic packet `Serializer` type used within netstack3.
pub trait DynamicNetworkSerializer: DynamicSerializer<NetworkSerializationContext> {}
impl<S: DynamicSerializer<NetworkSerializationContext>> DynamicNetworkSerializer for S {}

/// The specific dynamic packet `PartialSerializer` type used within netstack3.
pub trait DynamicNetworkPartialSerializer:
    DynamicPartialSerializer<NetworkSerializationContext>
{
}
impl<S: DynamicPartialSerializer<NetworkSerializationContext>> DynamicNetworkPartialSerializer
    for S
{
}

/// Networking protocols that support checksum offloading.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OffloadableProtocol {
    /// No protocol.
    #[default]
    None,
    /// Protocol does not support checksum offloading.
    NotOffloadable,
    /// Transmission Control Protocol.
    Tcp,
    /// User Datagram Protocol.
    Udp,
    /// Internet Protocol version 4.
    Ipv4,
    /// Internet Protocol version 6.
    Ipv6,
    /// Ethernet Frame.
    Ethernet,
}

bitflags! {
    /// Bitmask for networking protocols that support checksum offloading.
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct OffloadableProtocols: u8 {
        /// Not an offloadable protocol.
        const NOT_OFFLOADABLE = 1 << 0;
        /// Transmission Control Protocol.
        const TCP = 1 << 1;
        /// User Datagram Protocol.
        const UDP = 1 << 2;
        /// Internet Protocol version 4.
        const IPV4 = 1 << 3;
        /// Internet Protocol version 6.
        const IPV6 = 1 << 4;
        /// Ethernet Frame.
        const ETHERNET = 1 << 5;
    }
}

impl From<OffloadableProtocol> for OffloadableProtocols {
    fn from(p: OffloadableProtocol) -> Self {
        match p {
            OffloadableProtocol::None => Self::empty(),
            OffloadableProtocol::NotOffloadable => Self::NOT_OFFLOADABLE,
            OffloadableProtocol::Tcp => Self::TCP,
            OffloadableProtocol::Udp => Self::UDP,
            OffloadableProtocol::Ipv4 => Self::IPV4,
            OffloadableProtocol::Ipv6 => Self::IPV6,
            OffloadableProtocol::Ethernet => Self::ETHERNET,
        }
    }
}

bitflags! {
    /// Indicates that a device supports protocol-specific checksum offloading.
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    pub struct ProtocolSpecificOffloadSpec: u8 {
        /// Ethernet frame with IPv4 header (without options) and UDP payload.
        const ETH_IPV4_UDP = 1 << 0;
        /// Ethernet frame with IPv4 header (without options) and TCP payload.
        const ETH_IPV4_TCP = 1 << 1;
        /// Ethernet frame with IPv6 header (without extension headers) and UDP
        /// payload.
        const ETH_IPV6_UDP = 1 << 2;
        /// Ethernet frame with IPv6 header (without extension headers) and TCP
        /// payload.
        const ETH_IPV6_TCP = 1 << 3;
    }
}

impl ProtocolSpecificOffloadSpec {
    /// Creates a `ProtocolSpecificOffloadSpec` for devices that support
    /// protocol-specific checksum offloading for UDP and TCP packets over IPv4.
    ///
    /// This spec does not match when the IPv4 header contains options.
    pub fn tcp_or_udp_over_ipv4() -> Self {
        Self::ETH_IPV4_UDP | Self::ETH_IPV4_TCP
    }

    /// Creates a `ProtocolSpecificOffloadSpec` for devices that support
    /// protocol-specific checksum offloading for UDP and TCP packets over IPv6.
    ///
    /// This spec does not match when IPv6 contains extension headers.
    pub fn tcp_or_udp_over_ipv6() -> Self {
        Self::ETH_IPV6_UDP | Self::ETH_IPV6_TCP
    }

    /// Creates a `ProtocolSpecificOffloadSpec` that matches if either this spec
    /// or the other spec matches.
    fn or(self, other: Self) -> Self {
        self | other
    }

    /// Returns true if the `current` protocols match this spec.
    fn matches(&self, current: OffloadableProtocols) -> bool {
        use OffloadableProtocols as P;
        match current {
            f if f == (P::ETHERNET | P::IPV4 | P::UDP) => self.contains(Self::ETH_IPV4_UDP),
            f if f == (P::ETHERNET | P::IPV4 | P::TCP) => self.contains(Self::ETH_IPV4_TCP),
            f if f == (P::ETHERNET | P::IPV6 | P::UDP) => self.contains(Self::ETH_IPV6_UDP),
            f if f == (P::ETHERNET | P::IPV6 | P::TCP) => self.contains(Self::ETH_IPV6_TCP),
            _ => false,
        }
    }
}

/// Indicates that a device supports generic checksum offloading.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct GenericOffloadSpec;

/// Describes the checksum offloading capabilities available during serialization.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChecksumOffloadSpec {
    /// If `Some`, the device supports protocol-specific checksum offloading.
    protocol_specific: Option<ProtocolSpecificOffloadSpec>,
    /// If `Some`, the device supports generic checksum offloading.
    generic: Option<GenericOffloadSpec>,
}

impl ChecksumOffloadSpec {
    /// Creates a `ChecksumOffloadSpec` for a device that does not support any
    /// checksum offloading.
    pub fn none() -> Self {
        Self { protocol_specific: None, generic: None }
    }

    /// Creates a `ChecksumOffloadSpec` for a device that supports protocol-specific
    /// checksum offloading for the given protocols.
    pub fn protocol_specific(spec: ProtocolSpecificOffloadSpec) -> Self {
        Self { protocol_specific: Some(spec), generic: None }
    }

    /// Creates a `ChecksumOffloadSpec` for a device that supports generic
    /// checksum offloading.
    pub fn generic() -> Self {
        Self { protocol_specific: None, generic: Some(GenericOffloadSpec::default()) }
    }

    /// Creates a `ChecksumOffloadSpec` that matches any of the given specs.
    pub fn any<I: IntoIterator<Item = Self>>(specs: I) -> Self {
        specs.into_iter().fold(Self::none(), |acc, spec| Self {
            protocol_specific: match (acc.protocol_specific, spec.protocol_specific) {
                (Some(acc), Some(spec)) => Some(acc.or(spec)),
                (Some(acc), None) => Some(acc),
                (None, Some(spec)) => Some(spec),
                (None, None) => None,
            },
            generic: acc.generic.or(spec.generic),
        })
    }
}

impl Inspectable for ChecksumOffloadSpec {
    fn record<I: Inspector>(&self, inspector: &mut I) {
        inspector.record_bool("TxChecksumOffloadGeneric", self.generic.is_some());
        if let Some(protocol_specific) = &self.protocol_specific {
            inspector.record_child("TxChecksumOffloadProtocolSpecific", |inspector| {
                inspector.record_bool(
                    "EthIpv4Udp",
                    protocol_specific.contains(ProtocolSpecificOffloadSpec::ETH_IPV4_UDP),
                );
                inspector.record_bool(
                    "EthIpv4Tcp",
                    protocol_specific.contains(ProtocolSpecificOffloadSpec::ETH_IPV4_TCP),
                );
                inspector.record_bool(
                    "EthIpv6Udp",
                    protocol_specific.contains(ProtocolSpecificOffloadSpec::ETH_IPV6_UDP),
                );
                inspector.record_bool(
                    "EthIpv6Tcp",
                    protocol_specific.contains(ProtocolSpecificOffloadSpec::ETH_IPV6_TCP),
                );
            });
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProtocolStackInfo {
    // The offset in bytes from the start of the buffer to the start of the
    // current packet's header, if it fits in a u16.
    header_offset: Option<u16>,
    // The current protocol stack.
    protocols: OffloadableProtocols,
}

impl Default for ProtocolStackInfo {
    fn default() -> Self {
        Self { header_offset: Some(0), protocols: OffloadableProtocols::empty() }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ChecksumOffloadState {
    spec: ChecksumOffloadSpec,
    stack_info: ProtocolStackInfo,
}

impl ChecksumOffloadState {
    fn new(spec: ChecksumOffloadSpec) -> Self {
        Self { spec, stack_info: Default::default() }
    }

    /// Updates the checksum offload state for the given protocol and the
    /// constraints of the encapsulating context.
    ///
    /// Returns a value that can be passed to `restore` to restore the previous
    /// state.
    fn update(
        &mut self,
        protocol: OffloadableProtocol,
        constraints: &PacketConstraints,
    ) -> ProtocolStackInfo {
        let previous = self.stack_info.clone();

        let ProtocolStackInfo { header_offset, protocols } = &mut self.stack_info;

        // A `header_offset` value of `None` is sticky: it will be `None` for
        // all subsequent inner protocols and will remain so until an earlier
        // state is `restore`d.
        *header_offset = header_offset.and_then(|_| constraints.header_len().try_into().ok());

        if protocol == OffloadableProtocol::None {
            return previous;
        }
        let protocol = protocol.into();
        if protocols.contains(protocol) {
            // If we encounter a duplicate protocol in the stack, then we flag
            // that protocol-specific offloading is not available from that
            // point on. Like a `header_offset` of `None`, this value will be
            // retained until an earlier state is `restore`d.
            protocols.insert(OffloadableProtocol::NotOffloadable.into());
        } else {
            protocols.insert(protocol);
        }

        previous
    }

    /// Restores the previous checksum offload state.
    fn restore(&mut self, previous: ProtocolStackInfo) {
        self.stack_info = previous;
    }

    fn try_offload(&self, csum_offset: u16) -> Option<ChecksumOffloadResult> {
        let ProtocolStackInfo { header_offset, protocols } = self.stack_info;

        // We prefer generic offloading over protocol-specific offloading where
        // both are available to be consistent with Linux, which has been trying
        // to move toward generic offloading.
        if let Some(start) = self.spec.generic.as_ref().and(header_offset) {
            Some(ChecksumOffloadResult::Generic(PartialChecksum { start, offset: csum_offset }))
        } else if self
            .spec
            .protocol_specific
            .as_ref()
            .map(|spec| spec.matches(protocols))
            .unwrap_or(false)
        {
            Some(ChecksumOffloadResult::ProtocolSpecific(protocols))
        } else {
            None
        }
    }
}

/// Describes a partial checksum whose full checksum will be offloaded. The full checksum
/// must be computed by summing from `start` (the offset in bytes from the start of the
/// outermost packet header) to the end of the outermost packet and then placing the result
/// at `start + offset`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartialChecksum {
    /// The offset in bytes from the start of the outermost packet header to the start of the
    /// checksum.
    pub start: u16,
    /// The offset in bytes from the start of the checksum to the field that it replaces.
    pub offset: u16,
}

/// Describes the checksum offloading capability used during serialization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChecksumOffloadResult {
    /// Protocol-specific checksum offloading was utilized.
    ProtocolSpecific(OffloadableProtocols),
    /// Generic checksum offloading was utilized, producing a partial checksum.
    Generic(PartialChecksum),
}

/// A concrete serialization context for the entire network stack.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkSerializationContext {
    csum_offload_state: ChecksumOffloadState,
    /// Indicates whether or not checksum offloading capabilities have been
    /// utilized yet in the current serialization operation. Because checksum
    /// offloading can only be performed once per packet, a value of `Some`
    /// prevents checksum offloading from being performed multiple times.
    csum_offload_result: Option<ChecksumOffloadResult>,
}

impl NetworkSerializationContext {
    /// Creates a new `NetworkSerializationContext` with the given checksum offload capabilities.
    pub fn new(csum_offload_spec: ChecksumOffloadSpec) -> Self {
        Self {
            csum_offload_state: ChecksumOffloadState::new(csum_offload_spec),
            csum_offload_result: None,
        }
    }

    fn transport_checksum_action(&mut self, csum_offset: u16) -> TransportChecksumAction {
        if self.csum_offload_result.is_some() {
            // TODO(https://fxbug.dev/527140547): implement Local Checksum
            // Offload (LCO) for offloading outer checksums of encapsulated
            // packets.
            TransportChecksumAction::ComputeFull
        } else {
            self.csum_offload_result = self.csum_offload_state.try_offload(csum_offset);
            self.csum_offload_result
                .as_ref()
                .map(|_| TransportChecksumAction::ComputePartial)
                .unwrap_or(TransportChecksumAction::ComputeFull)
        }
    }

    /// Returns the result of the checksum offloading operation for the current
    /// packet, if any.
    pub fn csum_offload_result(self) -> Option<ChecksumOffloadResult> {
        self.csum_offload_result
    }
}

impl SerializationContext for NetworkSerializationContext {
    type ContextState = OffloadableProtocol;

    fn serialize_nested<O: PacketBuilder<Self>, R>(
        &mut self,
        outer: &O,
        constraints: PacketConstraints,
        serialize_fn: impl FnOnce(&mut Self, PacketConstraints) -> R,
    ) -> R {
        let previous_state = self.csum_offload_state.update(outer.context_state(), &constraints);
        let result = serialize_fn(self, constraints);
        self.csum_offload_state.restore(previous_state);
        result
    }
}

impl EthernetSerializationContext for NetworkSerializationContext {
    fn envelope_to_state(_envelope: EthernetEnvelope) -> Self::ContextState {
        OffloadableProtocol::Ethernet
    }
}

impl<I: IpExt> IpSerializationContext<I> for NetworkSerializationContext {
    fn envelope_to_state(envelope: IpEnvelope<I>) -> Self::ContextState {
        I::map_ip_in(
            IpInvariant(envelope),
            |IpInvariant(envelope)| {
                if envelope.has_options {
                    OffloadableProtocol::NotOffloadable
                } else {
                    OffloadableProtocol::Ipv4
                }
            },
            |IpInvariant(envelope)| {
                if envelope.has_options {
                    OffloadableProtocol::NotOffloadable
                } else {
                    OffloadableProtocol::Ipv6
                }
            },
        )
    }
}

impl IcmpSerializationContext for NetworkSerializationContext {
    fn envelope_to_state(_envelope: IcmpEnvelope) -> Self::ContextState {
        OffloadableProtocol::NotOffloadable
    }
}

const_assert!(packet_formats::udp::CHECKSUM_OFFSET <= u16::MAX as usize);
const UDP_CHECKSUM_OFFSET: u16 = packet_formats::udp::CHECKSUM_OFFSET as u16;

impl UdpSerializationContext for NetworkSerializationContext {
    fn envelope_to_state(_envelope: UdpEnvelope) -> Self::ContextState {
        OffloadableProtocol::Udp
    }

    fn checksum_action(&mut self) -> TransportChecksumAction {
        self.transport_checksum_action(UDP_CHECKSUM_OFFSET)
    }
}

const_assert!(packet_formats::tcp::CHECKSUM_OFFSET <= u16::MAX as usize);
const TCP_CHECKSUM_OFFSET: u16 = packet_formats::tcp::CHECKSUM_OFFSET as u16;

impl TcpSerializationContext for NetworkSerializationContext {
    fn envelope_to_state(_envelope: TcpEnvelope) -> Self::ContextState {
        OffloadableProtocol::Tcp
    }

    fn checksum_action(&mut self) -> TransportChecksumAction {
        self.transport_checksum_action(TCP_CHECKSUM_OFFSET)
    }
}

/// An indication of the checksums offloaded, if any, for a packet received from
/// a device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChecksumRxOffloading {
    /// The device offloaded zero or more checksums.
    ///
    /// `Some(n)` can only be used to describe offloading of TCP and UDP
    /// checksums.
    Offloaded(Option<NonZeroU16>),
    /// The device requires no checksum verification on packet ingress.
    ///
    /// NOTE: only intended to be used by the loopback interface.
    FullyOffloaded,
}

impl Default for ChecksumRxOffloading {
    fn default() -> Self {
        ChecksumRxOffloading::Offloaded(None)
    }
}

/// Context for parsing network packets in netstack3.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NetworkParsingContext {
    /// Hardware checksum offloading context.
    checksum_offload: ChecksumRxOffloading,
    verified_checksum_count: u16,
}

impl NetworkParsingContext {
    /// Creates a new `NetworkParsingContext`.
    pub fn new(checksum_offload: ChecksumRxOffloading) -> Self {
        NetworkParsingContext { checksum_offload, verified_checksum_count: 0 }
    }

    /// Returns the checksum offload status.
    pub fn checksum_offload(&self) -> ChecksumRxOffloading {
        self.checksum_offload
    }

    /// Returns the count of transport-layer checksums that were actually
    /// verified.
    ///
    /// Note that this only counts actual verifications and does not include
    /// skipped checksums.
    pub fn verified_checksum_count(&self) -> u16 {
        self.verified_checksum_count
    }

    /// Verifies a checksum using `f` if needed.
    ///
    /// If checksum verification should be skipped according to the offload
    /// configuration, `f` is not called. Otherwise, `f` is called and the
    /// verified checksum count is incremented on success.
    fn verify_checksum_if_needed_inner<E>(
        &mut self,
        f: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        match self.checksum_offload {
            ChecksumRxOffloading::FullyOffloaded => Ok(()),
            ChecksumRxOffloading::Offloaded(Some(n)) => {
                self.checksum_offload =
                    ChecksumRxOffloading::Offloaded(NonZeroU16::new(n.get() - 1));
                Ok(())
            }
            ChecksumRxOffloading::Offloaded(None) => match f() {
                Ok(()) => {
                    self.verified_checksum_count = self.verified_checksum_count.saturating_add(1);
                    Ok(())
                }
                Err(e) => Err(e),
            },
        }
    }
}

impl UdpParseContext for &mut NetworkParsingContext {
    fn verify_checksum_if_needed<E>(&mut self, f: impl FnOnce() -> Result<(), E>) -> Result<(), E> {
        self.verify_checksum_if_needed_inner(f)
    }
}

impl TcpParseContext for &mut NetworkParsingContext {
    fn verify_checksum_if_needed<E>(&mut self, f: impl FnOnce() -> Result<(), E>) -> Result<(), E> {
        self.verify_checksum_if_needed_inner(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use assert_matches::assert_matches;
    use core::num::NonZeroU16;
    use net_types::ethernet::Mac;
    use net_types::ip::{IpAddress, IpVersionMarker, Ipv4, Ipv4Addr, Ipv6Addr};
    use packet::{
        Buf, FragmentedBytesMut, FromRaw, NestablePacketBuilder, NestableSerializer, PacketBuilder,
        PacketConstraints, ParseBuffer, SerializeTarget, Serializer,
    };
    use packet_formats::error::ParseError;
    use packet_formats::ethernet::{
        EtherType, EthernetFrame, EthernetFrameBuilder, EthernetFrameLengthCheck,
    };
    use packet_formats::ip::{IpPacket, IpProto, Ipv4Proto, Ipv6Proto};
    use packet_formats::ipv4::options::Ipv4Option;
    use packet_formats::ipv4::{Ipv4Packet, Ipv4PacketBuilder, Ipv4PacketBuilderWithOptions};
    use packet_formats::ipv6::ext_hdrs::{
        ExtensionHeaderOptionAction, HopByHopOption, HopByHopOptionData,
    };
    use packet_formats::ipv6::{Ipv6PacketBuilder, Ipv6PacketBuilderWithHbhOptions};
    use packet_formats::tcp::TcpSegmentBuilder;
    use packet_formats::udp::{
        HEADER_BYTES as UDP_HEADER_BYTES, UdpPacket, UdpPacketBuilder, UdpPacketRaw, UdpParseArgs,
    };
    use test_case::test_case;

    const SRC_MAC: Mac = Mac::new([0, 1, 2, 3, 4, 5]);
    const DST_MAC: Mac = Mac::new([6, 7, 8, 9, 10, 11]);
    const SRC_IP_V4: Ipv4Addr = Ipv4Addr::new([192, 168, 0, 1]);
    const DST_IP_V4: Ipv4Addr = Ipv4Addr::new([192, 168, 0, 2]);
    const SRC_IP_V6: Ipv6Addr = Ipv6Addr::new([0, 0, 0, 0, 0, 0, 0, 1]);
    const DST_IP_V6: Ipv6Addr = Ipv6Addr::new([0, 0, 0, 0, 0, 0, 0, 2]);
    const SRC_PORT: u16 = 1234;
    const DST_PORT: u16 = 5678;

    #[test_case(
        UdpPacketBuilder::new(
            SRC_IP_V4,
            DST_IP_V4,
            NonZeroU16::new(SRC_PORT),
            NonZeroU16::new(DST_PORT).unwrap(),
        ),
        IpProto::Udp ; "udp"
    )]
    #[test_case(
        TcpSegmentBuilder::new(
            SRC_IP_V4,
            DST_IP_V4,
            NonZeroU16::new(SRC_PORT).unwrap(),
            NonZeroU16::new(DST_PORT).unwrap(),
            123,
            None,
            1000,
        ),
        IpProto::Tcp ; "tcp"
    )]
    fn ipv4_no_options_csum_offload(
        transport_builder: impl PacketBuilder<NetworkSerializationContext> + core::fmt::Debug,
        ip_proto: IpProto,
    ) {
        let mut payload = [0u8; 100];
        let ip = Ipv4PacketBuilder::new(SRC_IP_V4, DST_IP_V4, 64, Ipv4Proto::Proto(ip_proto));
        let ethernet = EthernetFrameBuilder::new(SRC_MAC, DST_MAC, EtherType::Ipv4, 0);

        let serializer =
            Buf::new(&mut payload[..], ..).wrap_in(transport_builder).wrap_in(ip).wrap_in(ethernet);

        let mut context = NetworkSerializationContext::new(ChecksumOffloadSpec::protocol_specific(
            ProtocolSpecificOffloadSpec::tcp_or_udp_over_ipv4(),
        ));
        let _ = serializer.serialize_vec_outer(&mut context).expect("serialization should succeed");

        let expected_protocol = match ip_proto {
            IpProto::Udp => OffloadableProtocol::Udp,
            IpProto::Tcp => OffloadableProtocol::Tcp,
            _ => panic!("invalid proto"),
        };
        assert_eq!(
            context.csum_offload_result(),
            Some(ChecksumOffloadResult::ProtocolSpecific(
                OffloadableProtocols::ETHERNET
                    | OffloadableProtocols::IPV4
                    | expected_protocol.into()
            ))
        );
    }

    #[test_case(
        UdpPacketBuilder::new(
            SRC_IP_V4,
            DST_IP_V4,
            NonZeroU16::new(SRC_PORT),
            NonZeroU16::new(DST_PORT).unwrap(),
        ),
        IpProto::Udp ; "udp"
    )]
    #[test_case(
        TcpSegmentBuilder::new(
            SRC_IP_V4,
            DST_IP_V4,
            NonZeroU16::new(SRC_PORT).unwrap(),
            NonZeroU16::new(DST_PORT).unwrap(),
            123,
            None,
            1000,
        ),
        IpProto::Tcp ; "tcp"
    )]
    fn ipv4_with_options_no_csum_offload(
        transport_builder: impl PacketBuilder<NetworkSerializationContext> + core::fmt::Debug,
        ip_proto: IpProto,
    ) {
        let mut payload = [0u8; 100];
        let ip = Ipv4PacketBuilder::new(SRC_IP_V4, DST_IP_V4, 64, Ipv4Proto::Proto(ip_proto));
        let options = [Ipv4Option::RouterAlert { data: 0 }];
        let ip_with_options = Ipv4PacketBuilderWithOptions::new(ip, &options).unwrap();
        let ethernet = EthernetFrameBuilder::new(SRC_MAC, DST_MAC, EtherType::Ipv4, 0);

        let serializer = Buf::new(&mut payload[..], ..)
            .wrap_in(transport_builder)
            .wrap_in(ip_with_options)
            .wrap_in(ethernet);

        let mut context = NetworkSerializationContext::new(ChecksumOffloadSpec::protocol_specific(
            ProtocolSpecificOffloadSpec::tcp_or_udp_over_ipv4(),
        ));
        let _ = serializer.serialize_vec_outer(&mut context).expect("serialization should succeed");

        assert_eq!(context.csum_offload_result(), None);
    }

    #[test_case(
        UdpPacketBuilder::new(
            SRC_IP_V6,
            DST_IP_V6,
            NonZeroU16::new(SRC_PORT),
            NonZeroU16::new(DST_PORT).unwrap(),
        ),
        IpProto::Udp ; "udp"
    )]
    #[test_case(
        TcpSegmentBuilder::new(
            SRC_IP_V6,
            DST_IP_V6,
            NonZeroU16::new(SRC_PORT).unwrap(),
            NonZeroU16::new(DST_PORT).unwrap(),
            123,
            None,
            1000,
        ),
        IpProto::Tcp ; "tcp"
    )]
    fn ipv6_no_extensions_csum_offload(
        transport_builder: impl PacketBuilder<NetworkSerializationContext> + core::fmt::Debug,
        ip_proto: IpProto,
    ) {
        let mut payload = [0u8; 100];
        let ip = Ipv6PacketBuilder::new(SRC_IP_V6, DST_IP_V6, 64, Ipv6Proto::Proto(ip_proto));
        let ethernet = EthernetFrameBuilder::new(SRC_MAC, DST_MAC, EtherType::Ipv6, 0);

        let serializer =
            Buf::new(&mut payload[..], ..).wrap_in(transport_builder).wrap_in(ip).wrap_in(ethernet);

        let mut context = NetworkSerializationContext::new(ChecksumOffloadSpec::protocol_specific(
            ProtocolSpecificOffloadSpec::tcp_or_udp_over_ipv6(),
        ));
        let _ = serializer.serialize_vec_outer(&mut context).expect("serialization should succeed");

        let expected_protocol = match ip_proto {
            IpProto::Udp => OffloadableProtocol::Udp,
            IpProto::Tcp => OffloadableProtocol::Tcp,
            _ => panic!("invalid proto"),
        };
        assert_eq!(
            context.csum_offload_result(),
            Some(ChecksumOffloadResult::ProtocolSpecific(
                OffloadableProtocols::ETHERNET
                    | OffloadableProtocols::IPV6
                    | expected_protocol.into()
            ))
        );
    }

    #[test_case(
        UdpPacketBuilder::new(
            SRC_IP_V6,
            DST_IP_V6,
            NonZeroU16::new(SRC_PORT),
            NonZeroU16::new(DST_PORT).unwrap(),
        ),
        IpProto::Udp ; "udp"
    )]
    #[test_case(
        TcpSegmentBuilder::new(
            SRC_IP_V6,
            DST_IP_V6,
            NonZeroU16::new(SRC_PORT).unwrap(),
            NonZeroU16::new(DST_PORT).unwrap(),
            123,
            None,
            1000,
        ),
        IpProto::Tcp ; "tcp"
    )]
    fn ipv6_with_extension_hdrs_no_csum_offload(
        transport_builder: impl PacketBuilder<NetworkSerializationContext> + core::fmt::Debug,
        ip_proto: IpProto,
    ) {
        let mut payload = [0u8; 100];
        let ip = Ipv6PacketBuilder::new(SRC_IP_V6, DST_IP_V6, 64, Ipv6Proto::Proto(ip_proto));
        let options = [HopByHopOption {
            action: ExtensionHeaderOptionAction::SkipAndContinue,
            mutable: false,
            data: HopByHopOptionData::RouterAlert { data: 0 },
        }];
        let ip_with_options = Ipv6PacketBuilderWithHbhOptions::new(ip, options).unwrap();
        let ethernet = EthernetFrameBuilder::new(SRC_MAC, DST_MAC, EtherType::Ipv6, 0);

        let serializer = Buf::new(&mut payload[..], ..)
            .wrap_in(transport_builder)
            .wrap_in(ip_with_options)
            .wrap_in(ethernet);

        let mut context = NetworkSerializationContext::new(ChecksumOffloadSpec::protocol_specific(
            ProtocolSpecificOffloadSpec::tcp_or_udp_over_ipv6(),
        ));
        let _ = serializer.serialize_vec_outer(&mut context).expect("serialization should succeed");

        assert_eq!(context.csum_offload_result(), None);
    }

    #[test]
    fn generic_csum_offload_preferred_over_protocol_specific() {
        let mut payload = [0u8; 100];
        let udp = UdpPacketBuilder::new(
            SRC_IP_V4,
            DST_IP_V4,
            NonZeroU16::new(SRC_PORT),
            NonZeroU16::new(DST_PORT).unwrap(),
        );
        let ip = Ipv4PacketBuilder::new(SRC_IP_V4, DST_IP_V4, 64, Ipv4Proto::Proto(IpProto::Udp));
        let ethernet = EthernetFrameBuilder::new(SRC_MAC, DST_MAC, EtherType::Ipv4, 0);

        let serializer = Buf::new(&mut payload[..], ..).wrap_in(udp).wrap_in(ip).wrap_in(ethernet);

        let mut context = NetworkSerializationContext::new(ChecksumOffloadSpec::any([
            ChecksumOffloadSpec::generic(),
            ChecksumOffloadSpec::protocol_specific(
                ProtocolSpecificOffloadSpec::tcp_or_udp_over_ipv4(),
            ),
        ]));
        let _ = serializer.serialize_vec_outer(&mut context).expect("serialization should succeed");

        // We expect generic offload to be preferred.
        assert_matches!(context.csum_offload_result(), Some(ChecksumOffloadResult::Generic(_)));
    }

    #[derive(Debug)]
    struct TestPacketBuilder {
        header_len: usize,
    }
    impl NestablePacketBuilder for TestPacketBuilder {
        fn constraints(&self) -> PacketConstraints {
            PacketConstraints::new(self.header_len, 0, 0, usize::MAX)
        }
    }
    impl PacketBuilder<NetworkSerializationContext> for TestPacketBuilder {
        fn context_state(&self) -> OffloadableProtocol {
            OffloadableProtocol::NotOffloadable
        }
        fn serialize(
            &self,
            _context: &mut NetworkSerializationContext,
            _target: &mut SerializeTarget<'_>,
            _body: FragmentedBytesMut<'_, '_>,
        ) {
            // Do nothing.
        }
    }

    #[test_case(
        UdpPacketBuilder::new(
            SRC_IP_V4,
            DST_IP_V4,
            NonZeroU16::new(SRC_PORT),
            NonZeroU16::new(DST_PORT).unwrap(),
        ),
        IpProto::Udp,
        UDP_CHECKSUM_OFFSET ; "udp"
    )]
    #[test_case(
        TcpSegmentBuilder::new(
            SRC_IP_V4,
            DST_IP_V4,
            NonZeroU16::new(SRC_PORT).unwrap(),
            NonZeroU16::new(DST_PORT).unwrap(),
            123,
            None,
            1000,
        ),
        IpProto::Tcp,
        TCP_CHECKSUM_OFFSET ; "tcp"
    )]
    fn generic_csum_offload(
        transport_builder: impl PacketBuilder<NetworkSerializationContext> + core::fmt::Debug,
        ip_proto: IpProto,
        expected_csum_offset: u16,
    ) {
        let mut payload = [0u8; 100];
        let test_packet = TestPacketBuilder { header_len: 10 };
        let ip = Ipv4PacketBuilder::new(SRC_IP_V4, DST_IP_V4, 64, Ipv4Proto::Proto(ip_proto));
        let ethernet = EthernetFrameBuilder::new(SRC_MAC, DST_MAC, EtherType::Ipv4, 0);

        // Buf -> test_packet -> transport_builder -> ip -> ethernet.
        let serializer = Buf::new(&mut payload[..], ..)
            // We add an additional header inside the transport packet to ensure
            // that the correct `header_offset` is restored as we walk back up
            // the stack.
            .wrap_in(test_packet)
            .wrap_in(transport_builder)
            .wrap_in(ip)
            .wrap_in(ethernet);

        let mut context = NetworkSerializationContext::new(ChecksumOffloadSpec::generic());
        let _ = serializer.serialize_vec_outer(&mut context).expect("serialization should succeed");

        // Ethernet header (14) + Ipv4 header (20) = 34.
        assert_eq!(
            context.csum_offload_result(),
            Some(ChecksumOffloadResult::Generic(PartialChecksum {
                start: 34,
                offset: expected_csum_offset
            }))
        );
    }

    #[test]
    fn generic_csum_offload_disabled_on_overflow() {
        let mut payload = [0u8; 100];
        // Use a header length that exceeds u16::MAX (65535).
        let test_packet = TestPacketBuilder { header_len: 66000 };
        let udp = UdpPacketBuilder::new(
            SRC_IP_V4,
            DST_IP_V4,
            NonZeroU16::new(SRC_PORT),
            NonZeroU16::new(DST_PORT).unwrap(),
        );
        let ip = Ipv4PacketBuilder::new(SRC_IP_V4, DST_IP_V4, 64, Ipv4Proto::Proto(IpProto::Udp));
        let ethernet = EthernetFrameBuilder::new(SRC_MAC, DST_MAC, EtherType::Ipv4, 0);

        // Wrap test_packet *outside* UDP to make the starting byte of the UDP
        // header overflow a u16.
        // Buf -> udp -> ip -> test_packet -> ethernet.
        let serializer = Buf::new(&mut payload[..], ..)
            .wrap_in(udp)
            .wrap_in(ip)
            .wrap_in(test_packet)
            .wrap_in(ethernet);

        let mut context = NetworkSerializationContext::new(ChecksumOffloadSpec::generic());
        let _ = serializer.serialize_vec_outer(&mut context).expect("serialization should succeed");

        // Generic offload should be disabled because of overflow.
        assert_eq!(context.csum_offload_result(), None);
    }

    #[test]
    fn generic_csum_offload_enabled_with_inner_overflow() {
        let mut payload = [0u8; 100];
        // Use a header length that exceeds u16::MAX (65535).
        let test_packet = TestPacketBuilder { header_len: 66000 };
        let udp = UdpPacketBuilder::new(
            SRC_IP_V6,
            DST_IP_V6,
            NonZeroU16::new(SRC_PORT),
            NonZeroU16::new(DST_PORT).unwrap(),
        );
        let ethernet = EthernetFrameBuilder::new(SRC_MAC, DST_MAC, EtherType::Ipv6, 0);

        // Wrap test_packet *inside* UDP, bypassing IP to avoid IP size limits.
        // Buf -> test_packet -> udp -> ethernet.
        let serializer =
            Buf::new(&mut payload[..], ..).wrap_in(test_packet).wrap_in(udp).wrap_in(ethernet);

        let mut context = NetworkSerializationContext::new(ChecksumOffloadSpec::generic());
        let _ = serializer.serialize_vec_outer(&mut context).expect("serialization should succeed");

        // Generic offload should work despite the overflow inside the UDP
        // packet.
        assert_eq!(
            context.csum_offload_result(),
            Some(ChecksumOffloadResult::Generic(PartialChecksum {
                start: 14, // Ethernet header length.
                offset: UDP_CHECKSUM_OFFSET
            }))
        );
    }

    #[test]
    fn protocol_specific_csum_offload_with_size_limit() {
        let mut payload = [0u8; 100];
        let udp = UdpPacketBuilder::new(
            SRC_IP_V4,
            DST_IP_V4,
            NonZeroU16::new(SRC_PORT),
            NonZeroU16::new(DST_PORT).unwrap(),
        );
        let ip = Ipv4PacketBuilder::new(SRC_IP_V4, DST_IP_V4, 64, Ipv4Proto::Proto(IpProto::Udp));
        let ethernet = EthernetFrameBuilder::new(SRC_MAC, DST_MAC, EtherType::Ipv4, 0);

        // Buf -> udp -> with_size_limit -> ip -> ethernet.
        let serializer = Buf::new(&mut payload[..], ..)
            .wrap_in(udp)
            // Tests that intermediate protocol-less packet builders like
            // `LimitedSizePacketBuilder` don't break protocol-specific
            // offloading.
            .with_size_limit(1000)
            .wrap_in(ip)
            .wrap_in(ethernet);

        let mut context = NetworkSerializationContext::new(ChecksumOffloadSpec::protocol_specific(
            ProtocolSpecificOffloadSpec::tcp_or_udp_over_ipv4(),
        ));
        let _ = serializer.serialize_vec_outer(&mut context).expect("serialization should succeed");

        assert_eq!(
            context.csum_offload_result(),
            Some(ChecksumOffloadResult::ProtocolSpecific(
                OffloadableProtocols::ETHERNET
                    | OffloadableProtocols::IPV4
                    | OffloadableProtocols::UDP
            ))
        );
    }

    #[test]
    fn protocol_specific_csum_offload_duplicate_protocol() {
        let mut payload = [0u8; 100];
        let udp_inner = UdpPacketBuilder::new(
            SRC_IP_V4,
            DST_IP_V4,
            NonZeroU16::new(SRC_PORT),
            NonZeroU16::new(DST_PORT).unwrap(),
        );
        let ip_inner =
            Ipv4PacketBuilder::new(SRC_IP_V4, DST_IP_V4, 64, Ipv4Proto::Proto(IpProto::Udp));
        let udp_outer = UdpPacketBuilder::new(
            SRC_IP_V4,
            DST_IP_V4,
            NonZeroU16::new(SRC_PORT),
            NonZeroU16::new(DST_PORT).unwrap(),
        );
        let ip_outer =
            Ipv4PacketBuilder::new(SRC_IP_V4, DST_IP_V4, 64, Ipv4Proto::Proto(IpProto::Udp));
        let ethernet = EthernetFrameBuilder::new(SRC_MAC, DST_MAC, EtherType::Ipv4, 0);

        // Buf -> udp_inner -> ip_inner -> udp_outer -> ip_outer -> ethernet.
        let serializer = Buf::new(&mut payload[..], ..)
            .wrap_in(udp_inner)
            .wrap_in(ip_inner)
            .wrap_in(udp_outer)
            .wrap_in(ip_outer)
            .wrap_in(ethernet);

        let mut context = NetworkSerializationContext::new(ChecksumOffloadSpec::protocol_specific(
            ProtocolSpecificOffloadSpec::tcp_or_udp_over_ipv4(),
        ));
        let buf =
            serializer.serialize_vec_outer(&mut context).expect("serialization should succeed");

        // Protocol-specific offload should work for the outer UDP packet even
        // with the duplicate UDP packet.
        assert_eq!(
            context.csum_offload_result(),
            Some(ChecksumOffloadResult::ProtocolSpecific(
                OffloadableProtocols::ETHERNET
                    | OffloadableProtocols::IPV4
                    | OffloadableProtocols::UDP
            ))
        );

        let mut buf_ref = buf.as_ref();
        let eth = buf_ref
            .parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::Check)
            .expect("ethernet parse should succeed");
        let mut body = eth.body();
        let ip_out = body.parse::<Ipv4Packet<_>>().expect("outer ipv4 parse should succeed");

        // Parse outer UDP as raw (succeeds since it doesn't validate checksum).
        let mut outer_udp_bytes = ip_out.body();
        let udp_out_raw = outer_udp_bytes
            .parse_with::<_, UdpPacketRaw<_>>(IpVersionMarker::<Ipv4>::default())
            .expect("outer udp parse should succeed");

        // Try to validate outer UDP, which should fail checksum validation
        // because the checksum was offloaded.
        assert_eq!(
            UdpPacket::try_from_raw_with(
                udp_out_raw,
                UdpParseArgs::new(ip_out.src_ip(), ip_out.dst_ip())
            )
            .err(),
            Some(ParseError::Checksum),
        );

        let mut inner_ip_bytes = &ip_out.body()[UDP_HEADER_BYTES..];
        let ip_in =
            inner_ip_bytes.parse::<Ipv4Packet<_>>().expect("inner ipv4 parse should succeed");
        let mut body = ip_in.body();

        // This should succeed because inner UDP checksum was computed in
        // software.
        let _udp_in = body
            .parse_with::<_, UdpPacket<_>>(UdpParseArgs::new(ip_in.src_ip(), ip_in.dst_ip()))
            .expect("inner udp parse should succeed");
    }

    #[test]
    fn generic_csum_offload_duplicate_protocol() {
        let mut payload = [0u8; 100];
        let udp_inner = UdpPacketBuilder::new(
            SRC_IP_V4,
            DST_IP_V4,
            NonZeroU16::new(SRC_PORT),
            NonZeroU16::new(DST_PORT).unwrap(),
        );
        let ip_inner =
            Ipv4PacketBuilder::new(SRC_IP_V4, DST_IP_V4, 64, Ipv4Proto::Proto(IpProto::Udp));
        let udp_outer = UdpPacketBuilder::new(
            SRC_IP_V4,
            DST_IP_V4,
            NonZeroU16::new(SRC_PORT),
            NonZeroU16::new(DST_PORT).unwrap(),
        );
        let ip_outer =
            Ipv4PacketBuilder::new(SRC_IP_V4, DST_IP_V4, 64, Ipv4Proto::Proto(IpProto::Udp));
        let ethernet = EthernetFrameBuilder::new(SRC_MAC, DST_MAC, EtherType::Ipv4, 0);

        // Buf -> udp_inner -> ip_inner -> udp_outer -> ip_outer -> ethernet.
        let serializer = Buf::new(&mut payload[..], ..)
            .wrap_in(udp_inner)
            .wrap_in(ip_inner)
            .wrap_in(udp_outer)
            .wrap_in(ip_outer)
            .wrap_in(ethernet);

        let mut context = NetworkSerializationContext::new(ChecksumOffloadSpec::generic());
        let buf =
            serializer.serialize_vec_outer(&mut context).expect("serialization should succeed");

        // Generic offload should apply to the inner UDP packet.
        // Eth (14) + outer IPv4 (20) + UDP (8) + inner IPv4 (20) = 62.
        assert_eq!(
            context.csum_offload_result(),
            Some(ChecksumOffloadResult::Generic(PartialChecksum {
                start: 62,
                offset: UDP_CHECKSUM_OFFSET
            }))
        );

        let mut buf_ref = buf.as_ref();
        let eth = buf_ref
            .parse_with::<_, EthernetFrame<_>>(EthernetFrameLengthCheck::Check)
            .expect("ethernet parse should succeed");
        let mut body = eth.body();
        let ip_out = body.parse::<Ipv4Packet<_>>().expect("outer ipv4 parse should succeed");

        // Outer UDP checksum was computed in software, so parse should succeed.
        let mut outer_udp_bytes = ip_out.body();
        let _udp_out = outer_udp_bytes
            .parse_with::<_, UdpPacket<_>>(UdpParseArgs::new(ip_out.src_ip(), ip_out.dst_ip()))
            .expect("outer udp parse should succeed");

        let mut inner_ip_bytes = &ip_out.body()[UDP_HEADER_BYTES..];
        let ip_in =
            inner_ip_bytes.parse::<Ipv4Packet<_>>().expect("inner ipv4 parse should succeed");
        let mut body = ip_in.body();

        // Parse inner UDP as raw (succeeds since it doesn't validate checksum).
        let udp_in_raw = body
            .parse_with::<_, UdpPacketRaw<_>>(IpVersionMarker::<Ipv4>::default())
            .expect("inner udp parse should succeed");

        // Try to validate inner UDP, which should fail checksum validation
        // because the checksum was offloaded.
        assert_eq!(
            UdpPacket::try_from_raw_with(
                udp_in_raw,
                UdpParseArgs::new(ip_in.src_ip(), ip_in.dst_ip())
            )
            .err(),
            Some(ParseError::Checksum),
        );
    }

    fn build_udp_packet_invalid_csum<I: IpAddress>(
        src_ip: I,
        dst_ip: I,
        body: &mut [u8],
    ) -> Vec<u8> {
        let mut buf = Buf::new(body, ..)
            .wrap_in(UdpPacketBuilder::new(
                src_ip,
                dst_ip,
                NonZeroU16::new(1),
                NonZeroU16::new(2).unwrap(),
            ))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .as_ref()
            .to_vec();

        // Corrupt the checksum.
        buf[packet_formats::udp::CHECKSUM_OFFSET] ^= 0xFF;
        buf[packet_formats::udp::CHECKSUM_OFFSET + 1] ^= 0xFF;
        buf
    }

    /// Builds a UDP packet containing `nesting-1` nested UDP packets, all with
    /// invalid checksums.
    fn build_nested_udp_packets_invalid_csums<I: IpAddress>(
        src_ip: I,
        dst_ip: I,
        nesting: usize,
    ) -> Vec<u8> {
        let mut payload = alloc::vec![0u8; 100];
        for _ in 0..nesting {
            payload = build_udp_packet_invalid_csum(src_ip, dst_ip, &mut payload);
        }
        payload
    }

    #[test]
    fn checksum_rx_offloading_none() {
        let buf = build_nested_udp_packets_invalid_csums(SRC_IP_V4, DST_IP_V4, 1);

        // `None` offloads no checksums so we expect to be unable to parse any
        // UDP packets with invalid checksums.
        let mut ctx = NetworkParsingContext::new(ChecksumRxOffloading::Offloaded(None));
        let mut buf_ref: &[u8] = buf.as_ref();
        assert_eq!(
            buf_ref
                .parse_with::<_, UdpPacket<_>>(UdpParseArgs::with_context(
                    SRC_IP_V4, DST_IP_V4, &mut ctx
                ))
                .err(),
            Some(ParseError::Checksum)
        );
        assert_eq!(ctx.verified_checksum_count(), 0);
    }

    #[test]
    fn checksum_rx_offloading_fully_offloaded() {
        let mut buf = build_nested_udp_packets_invalid_csums(SRC_IP_V4, DST_IP_V4, 3);

        // `FullyOffloaded` offloads all checksums so we expect to be able to
        // parse an arbitrary number of UDP packets with invalid checksums.
        let mut ctx = NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded);
        for _ in 0..3 {
            let mut buf_ref: &[u8] = buf.as_ref();
            buf = buf_ref
                .parse_with::<_, UdpPacket<_>>(UdpParseArgs::with_context(
                    SRC_IP_V4, DST_IP_V4, &mut ctx,
                ))
                .expect("udp parse should succeed")
                .body()
                .to_vec();
        }
        assert_eq!(ctx.verified_checksum_count(), 0);
    }

    #[test]
    fn checksum_rx_offloading_offloaded() {
        let mut buf = build_nested_udp_packets_invalid_csums(SRC_IP_V4, DST_IP_V4, 3);

        // `Offloaded` indicates the number checksums not to verify so we expect
        // to be able to parse exactly two UDP packets with invalid checksums.
        let mut ctx = NetworkParsingContext::new(ChecksumRxOffloading::Offloaded(Some(
            NonZeroU16::new(2).unwrap(),
        )));
        for _ in 0..2 {
            let mut buf_ref: &[u8] = buf.as_ref();
            buf = buf_ref
                .parse_with::<_, UdpPacket<_>>(UdpParseArgs::with_context(
                    SRC_IP_V4, DST_IP_V4, &mut ctx,
                ))
                .expect("udp parse should succeed")
                .body()
                .to_vec();
        }
        let mut buf_ref: &[u8] = buf.as_ref();
        assert_eq!(
            buf_ref
                .parse_with::<_, UdpPacket<_>>(UdpParseArgs::with_context(
                    SRC_IP_V4, DST_IP_V4, &mut ctx
                ))
                .err(),
            Some(ParseError::Checksum)
        );
        assert_eq!(ctx.verified_checksum_count(), 0);
    }

    #[test]
    fn checksum_rx_offloading_verified_ok() {
        let mut payload = [0u8; 10];
        let buf = Buf::new(&mut payload[..], ..)
            .wrap_in(UdpPacketBuilder::new(
                SRC_IP_V4,
                DST_IP_V4,
                Some(NonZeroU16::new(1234).unwrap()),
                NonZeroU16::new(5678).unwrap(),
            ))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .as_ref()
            .to_vec();

        let mut ctx = NetworkParsingContext::new(ChecksumRxOffloading::Offloaded(None));
        let mut buf_ref: &[u8] = buf.as_ref();
        assert!(
            buf_ref
                .parse_with::<_, UdpPacket<_>>(UdpParseArgs::with_context(
                    SRC_IP_V4, DST_IP_V4, &mut ctx
                ))
                .is_ok()
        );
        assert_eq!(ctx.verified_checksum_count(), 1);
    }
}
