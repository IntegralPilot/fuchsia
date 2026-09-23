// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extensions for the fuchsia.net.matchers FIDL library.
//!
//! Note that this library as written is not meant for inclusion in the SDK. It
//! is only meant to be used in conjunction with a netstack that is compiled
//! against the same API level of the `fuchsia.net.matchers` FIDL library. This
//! library opts in to compile-time and runtime breakage when the FIDL library
//! is evolved in order to enforce that it is updated along with the FIDL
//! library itself.

use std::fmt::Debug;
use std::num::NonZeroU64;
use std::ops::RangeInclusive;

use fidl::marker::SourceBreaking;
use fidl_fuchsia_net_common as fnet;
use fidl_fuchsia_net_ext::{FromExt, IntoExt};
use fidl_fuchsia_net_interfaces_common as fnet_interfaces;
#[cfg(not(feature = "fdomain"))]
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_interfaces_ext_fdomain as fnet_interfaces_ext;
use fidl_fuchsia_net_matchers_common as fnet_matchers;
use net_types::ip::Ip;
use thiserror::Error;

/// Extension type for [`fnet_matchers::Interface`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Interface {
    Id(NonZeroU64),
    Name(fnet_interfaces::Name),
    PortClass(fnet_interfaces_ext::PortClass),
}

/// Errors when creating an [`Interface`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InterfaceError {
    #[error("interface matcher specified an invalid ID of 0")]
    ZeroId,
    #[error(transparent)]
    UnknownPortClass(fnet_interfaces_ext::UnknownPortClassError),
    #[error("interface union is of an unknown variant")]
    UnknownUnionVariant,
}

impl From<Interface> for fnet_matchers::Interface {
    fn from(matcher: Interface) -> Self {
        match matcher {
            Interface::Id(id) => Self::Id(id.get()),
            Interface::Name(name) => Self::Name(name),
            Interface::PortClass(port_class) => Self::PortClass(port_class.into()),
        }
    }
}

impl TryFrom<fnet_matchers::Interface> for Interface {
    type Error = InterfaceError;

    fn try_from(matcher: fnet_matchers::Interface) -> Result<Self, Self::Error> {
        match matcher {
            fnet_matchers::Interface::Id(id) => {
                let id = NonZeroU64::new(id).ok_or(InterfaceError::ZeroId)?;
                Ok(Self::Id(id))
            }
            fnet_matchers::Interface::Name(name) => Ok(Self::Name(name)),
            fnet_matchers::Interface::PortClass(port_class) => {
                port_class.try_into().map(Self::PortClass).map_err(InterfaceError::UnknownPortClass)
            }
            fnet_matchers::Interface::__SourceBreaking { .. } => {
                Err(InterfaceError::UnknownUnionVariant)
            }
        }
    }
}

/// Extension type for [`fnet_matchers::BoundInterface`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BoundInterface {
    Unbound,
    Bound(Interface),
}

/// Errors when creating an [`BoundInterface`].
#[derive(Debug, Error, PartialEq)]
pub enum BoundInterfaceError {
    #[error(transparent)]
    Interface(InterfaceError),
    #[error("interface union is of an unknown variant")]
    UnknownUnionVariant(u64),
}

impl From<BoundInterface> for fnet_matchers::BoundInterface {
    fn from(matcher: BoundInterface) -> Self {
        match matcher {
            BoundInterface::Unbound => fnet_matchers::BoundInterface::Unbound(fnet_matchers::Empty),
            BoundInterface::Bound(interface) => {
                fnet_matchers::BoundInterface::Bound(interface.into())
            }
        }
    }
}

impl TryFrom<fnet_matchers::BoundInterface> for BoundInterface {
    type Error = BoundInterfaceError;

    fn try_from(matcher: fnet_matchers::BoundInterface) -> Result<Self, Self::Error> {
        match matcher {
            fnet_matchers::BoundInterface::Unbound(fnet_matchers::Empty) => {
                Ok(BoundInterface::Unbound)
            }
            fnet_matchers::BoundInterface::Bound(interface) => Ok(BoundInterface::Bound(
                interface.try_into().map_err(|e| BoundInterfaceError::Interface(e))?,
            )),
            fnet_matchers::BoundInterface::__SourceBreaking { unknown_ordinal } => {
                Err(BoundInterfaceError::UnknownUnionVariant(unknown_ordinal))
            }
        }
    }
}

/// Extension type for the `Subnet` variant of [`fnet_matchers::Address`].
///
/// This type witnesses to the invariant that the prefix length of the subnet is
/// no greater than the number of bits in the IP address, and that no host bits
/// in the network address are set.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct Subnet(net_types::ip::SubnetEither);

/// Errors when creating a [`Subnet`].
#[derive(Debug, Error, PartialEq)]
pub enum SubnetError {
    #[error("prefix length of subnet is longer than number of bits in IP address")]
    PrefixTooLong,
    #[error("host bits are set in subnet network")]
    HostBitsSet,
}

impl From<Subnet> for net_types::ip::SubnetEither {
    fn from(subnet: Subnet) -> Self {
        let Subnet(subnet) = subnet;
        subnet
    }
}

impl From<net_types::ip::SubnetEither> for Subnet {
    fn from(subnet: net_types::ip::SubnetEither) -> Self {
        Self(subnet)
    }
}

impl From<Subnet> for fnet::Subnet {
    fn from(subnet: Subnet) -> Self {
        let Subnet(subnet) = subnet;
        let (addr, prefix_len) = subnet.net_prefix();

        Self { addr: fnet::IpAddress::from_ext(addr), prefix_len }
    }
}

impl TryFrom<fnet::Subnet> for Subnet {
    type Error = SubnetError;

    fn try_from(subnet: fnet::Subnet) -> Result<Self, Self::Error> {
        let fnet::Subnet { addr, prefix_len } = subnet;

        match net_types::ip::SubnetEither::new(addr.into_ext(), prefix_len) {
            Ok(inner) => Ok(Self(inner)),
            Err(err) => Err(match err {
                net_types::ip::SubnetError::PrefixTooLong => SubnetError::PrefixTooLong,
                net_types::ip::SubnetError::HostBitsSet => SubnetError::HostBitsSet,
            }),
        }
    }
}

impl Debug for Subnet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(subnet) = self;
        match subnet {
            net_types::ip::SubnetEither::V4(subnet) => subnet.fmt(f),
            net_types::ip::SubnetEither::V6(subnet) => subnet.fmt(f),
        }
    }
}

/// Extension type for [`fnet_matchers::AddressRange`].
///
/// This type witnesses that `start <= end`. (Comparisons are performed on the
/// numerical big-endian representation of the IP address.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressRange {
    V4(RangeInclusive<fnet::Ipv4Address>),
    V6(RangeInclusive<fnet::Ipv6Address>),
}

/// Errors when creating an [`AddressRange`].
#[derive(Debug, Error, PartialEq)]
pub enum AddressRangeError {
    #[error("invalid address range (start must be <= end)")]
    Invalid,
    #[error("address range start and end addresses are not the same IP family")]
    FamilyMismatch,
}

impl From<AddressRange> for fnet_matchers::AddressRange {
    fn from(range: AddressRange) -> Self {
        let (start, end) = match range {
            AddressRange::V4(range) => ((*range.start()).into_ext(), (*range.end()).into_ext()),
            AddressRange::V6(range) => ((*range.start()).into_ext(), (*range.end()).into_ext()),
        };

        Self { start, end }
    }
}

impl TryFrom<fnet_matchers::AddressRange> for AddressRange {
    type Error = AddressRangeError;

    fn try_from(range: fnet_matchers::AddressRange) -> Result<Self, Self::Error> {
        let fnet_matchers::AddressRange { start, end } = range;
        match (start, end) {
            (fnet::IpAddress::Ipv4(start), fnet::IpAddress::Ipv4(end)) => {
                if u32::from_be_bytes(start.addr) > u32::from_be_bytes(end.addr) {
                    Err(AddressRangeError::Invalid)
                } else {
                    Ok(Self::V4(start..=end))
                }
            }
            (fnet::IpAddress::Ipv6(start), fnet::IpAddress::Ipv6(end)) => {
                if u128::from_be_bytes(start.addr) > u128::from_be_bytes(end.addr) {
                    Err(AddressRangeError::Invalid)
                } else {
                    Ok(Self::V6(start..=end))
                }
            }
            _ => Err(AddressRangeError::FamilyMismatch),
        }
    }
}

impl AddressRange {
    /// Create a new [`AddressRange`] for a single address.
    pub fn new_single<I: Ip>(ip: I::Addr) -> Self {
        I::map_ip_in(
            ip,
            |ip| {
                let ip = ip.into_ext();
                Self::V4(ip..=ip)
            },
            |ip| {
                let ip = ip.into_ext();
                Self::V6(ip..=ip)
            },
        )
    }
}

/// Extension type for [`fnet_matchers::Address`].
#[derive(Clone, PartialEq, Eq)]
pub enum AddressMatcherType {
    Subnet(Subnet),
    Range(AddressRange),
}

/// Errors when creating an [`AddressMatcherType`].
#[derive(Debug, Error, PartialEq)]
pub enum AddressMatcherTypeError {
    #[error("AddressMatcher is of an unknown variant")]
    UnknownUnionVariant,
    #[error("subnet conversion error: {0}")]
    Subnet(SubnetError),
    #[error("address range conversion error: {0}")]
    AddressRange(AddressRangeError),
}

impl From<SubnetError> for AddressMatcherTypeError {
    fn from(value: SubnetError) -> Self {
        AddressMatcherTypeError::Subnet(value)
    }
}
impl From<AddressRangeError> for AddressMatcherTypeError {
    fn from(value: AddressRangeError) -> Self {
        AddressMatcherTypeError::AddressRange(value)
    }
}

impl From<AddressMatcherType> for fnet_matchers::AddressMatcherType {
    fn from(matcher: AddressMatcherType) -> Self {
        match matcher {
            AddressMatcherType::Subnet(subnet) => Self::Subnet(subnet.into()),
            AddressMatcherType::Range(range) => Self::Range(range.into()),
        }
    }
}

impl TryFrom<fnet_matchers::AddressMatcherType> for AddressMatcherType {
    type Error = AddressMatcherTypeError;

    fn try_from(matcher: fnet_matchers::AddressMatcherType) -> Result<Self, Self::Error> {
        match matcher {
            fnet_matchers::AddressMatcherType::Subnet(subnet) => {
                Ok(Self::Subnet(subnet.try_into()?))
            }
            fnet_matchers::AddressMatcherType::Range(range) => Ok(Self::Range(range.try_into()?)),
            fnet_matchers::AddressMatcherType::__SourceBreaking { .. } => {
                Err(AddressMatcherTypeError::UnknownUnionVariant)
            }
        }
    }
}

impl Debug for AddressMatcherType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddressMatcherType::Subnet(subnet) => subnet.fmt(f),
            AddressMatcherType::Range(address_range) => address_range.fmt(f),
        }
    }
}

/// Extension type for [`fnet_matchers::Address`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub matcher: AddressMatcherType,
    pub invert: bool,
}

/// Errors when creating an [`Address`].
#[derive(Debug, Error, PartialEq)]
pub enum AddressError {
    #[error("address matcher conversion failure: {0}")]
    AddressMatcherType(AddressMatcherTypeError),
}

impl From<AddressMatcherTypeError> for AddressError {
    fn from(value: AddressMatcherTypeError) -> Self {
        Self::AddressMatcherType(value)
    }
}

impl From<Address> for fnet_matchers::Address {
    fn from(matcher: Address) -> Self {
        let Address { matcher, invert } = matcher;
        Self { matcher: matcher.into(), invert }
    }
}

impl TryFrom<fnet_matchers::Address> for Address {
    type Error = AddressError;

    fn try_from(matcher: fnet_matchers::Address) -> Result<Self, Self::Error> {
        let fnet_matchers::Address { matcher, invert } = matcher;
        Ok(Self { matcher: matcher.try_into()?, invert })
    }
}

/// Extension type for [`fnet_matchers::BoundAddress`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundAddress {
    Unbound,
    Bound(Address),
}

/// Errors when creating an [`BoundAddress`].
#[derive(Debug, Error, PartialEq)]
pub enum BoundAddressError {
    #[error(transparent)]
    Address(AddressError),
    #[error("bound address union is of an unknown variant")]
    UnknownUnionVariant(u64),
}

impl From<BoundAddress> for fnet_matchers::BoundAddress {
    fn from(matcher: BoundAddress) -> Self {
        match matcher {
            BoundAddress::Unbound => fnet_matchers::BoundAddress::Unbound(fnet_matchers::Empty),
            BoundAddress::Bound(address) => fnet_matchers::BoundAddress::Bound(address.into()),
        }
    }
}

impl TryFrom<fnet_matchers::BoundAddress> for BoundAddress {
    type Error = BoundAddressError;

    fn try_from(matcher: fnet_matchers::BoundAddress) -> Result<Self, Self::Error> {
        match matcher {
            fnet_matchers::BoundAddress::Unbound(fnet_matchers::Empty) => Ok(BoundAddress::Unbound),
            fnet_matchers::BoundAddress::Bound(address) => {
                Ok(BoundAddress::Bound(address.try_into().map_err(BoundAddressError::Address)?))
            }
            fnet_matchers::BoundAddress::__SourceBreaking { unknown_ordinal } => {
                Err(BoundAddressError::UnknownUnionVariant(unknown_ordinal))
            }
        }
    }
}

/// Extension type for [`fnet_matchers::Port`].
///
/// This type witnesses to the invariant that `start <= end`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Port {
    range: RangeInclusive<u16>,
    invert: bool,
}

/// Extension type for [`fnet_matchers::BoundPort`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BoundPort {
    Unbound,
    Bound(Port),
}

/// Errors when creating an [`BoundPort`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BoundPortError {
    #[error(transparent)]
    Port(PortError),
    #[error("bound port union is of an unknown variant")]
    UnknownUnionVariant(u64),
}

impl From<BoundPort> for fnet_matchers::BoundPort {
    fn from(matcher: BoundPort) -> Self {
        match matcher {
            BoundPort::Unbound => fnet_matchers::BoundPort::Unbound(fnet_matchers::Empty),
            BoundPort::Bound(port) => fnet_matchers::BoundPort::Bound(port.into()),
        }
    }
}

impl TryFrom<fnet_matchers::BoundPort> for BoundPort {
    type Error = BoundPortError;

    fn try_from(matcher: fnet_matchers::BoundPort) -> Result<Self, Self::Error> {
        match matcher {
            fnet_matchers::BoundPort::Unbound(fnet_matchers::Empty) => Ok(BoundPort::Unbound),
            fnet_matchers::BoundPort::Bound(port) => {
                Ok(BoundPort::Bound(port.try_into().map_err(BoundPortError::Port)?))
            }
            fnet_matchers::BoundPort::__SourceBreaking { unknown_ordinal } => {
                Err(BoundPortError::UnknownUnionVariant(unknown_ordinal))
            }
        }
    }
}

/// Errors when creating a `Port`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PortError {
    #[error("invalid port range (start must be <= end)")]
    InvalidPortRange,
}

impl Port {
    pub fn new(start: u16, end: u16, invert: bool) -> Result<Self, PortError> {
        if start > end {
            return Err(PortError::InvalidPortRange);
        }
        Ok(Self { range: start..=end, invert })
    }

    /// Create a new [`Port`] for a single port.
    pub fn new_single(port: u16, invert: bool) -> Self {
        Self::new(port, port, invert).unwrap()
    }

    pub fn range(&self) -> &RangeInclusive<u16> {
        &self.range
    }

    pub fn start(&self) -> u16 {
        *self.range.start()
    }

    pub fn end(&self) -> u16 {
        *self.range.end()
    }

    pub fn invert(&self) -> bool {
        self.invert
    }
}

impl From<Port> for fnet_matchers::Port {
    fn from(matcher: Port) -> Self {
        let Port { range, invert } = matcher;
        Self { start: *range.start(), end: *range.end(), invert }
    }
}

impl TryFrom<fnet_matchers::Port> for Port {
    type Error = PortError;

    fn try_from(matcher: fnet_matchers::Port) -> Result<Self, Self::Error> {
        let fnet_matchers::Port { start, end, invert } = matcher;
        if start > end {
            return Err(PortError::InvalidPortRange);
        }
        Ok(Self { range: start..=end, invert })
    }
}

/// Extension type for [`fnet_matchers::PacketTransportProtocol`].
#[derive(Clone, PartialEq)]
pub enum TransportProtocol {
    Tcp { src_port: Option<Port>, dst_port: Option<Port> },
    Udp { src_port: Option<Port>, dst_port: Option<Port> },
    Icmp,
    Icmpv6,
}

/// Errors when creating a [`TransportProtocol`].
#[derive(Debug, Error, PartialEq)]
pub enum TransportProtocolError {
    #[error("invalid port: {0}")]
    Port(PortError),
    #[error("TransportProtocol is of an unknown variant")]
    UnknownUnionVariant,
}

impl From<PortError> for TransportProtocolError {
    fn from(value: PortError) -> Self {
        TransportProtocolError::Port(value)
    }
}

impl From<TransportProtocol> for fnet_matchers::PacketTransportProtocol {
    fn from(matcher: TransportProtocol) -> Self {
        match matcher {
            TransportProtocol::Tcp { src_port, dst_port } => Self::Tcp(fnet_matchers::TcpPacket {
                src_port: src_port.map(Into::into),
                dst_port: dst_port.map(Into::into),
                __source_breaking: SourceBreaking,
            }),
            TransportProtocol::Udp { src_port, dst_port } => Self::Udp(fnet_matchers::UdpPacket {
                src_port: src_port.map(Into::into),
                dst_port: dst_port.map(Into::into),
                __source_breaking: SourceBreaking,
            }),
            TransportProtocol::Icmp => Self::Icmp(fnet_matchers::IcmpPacket::default()),
            TransportProtocol::Icmpv6 => Self::Icmpv6(fnet_matchers::Icmpv6Packet::default()),
        }
    }
}

impl TryFrom<fnet_matchers::PacketTransportProtocol> for TransportProtocol {
    type Error = TransportProtocolError;

    fn try_from(matcher: fnet_matchers::PacketTransportProtocol) -> Result<Self, Self::Error> {
        match matcher {
            fnet_matchers::PacketTransportProtocol::Tcp(fnet_matchers::TcpPacket {
                src_port,
                dst_port,
                __source_breaking,
            }) => Ok(Self::Tcp {
                src_port: src_port.map(TryInto::try_into).transpose()?,
                dst_port: dst_port.map(TryInto::try_into).transpose()?,
            }),
            fnet_matchers::PacketTransportProtocol::Udp(fnet_matchers::UdpPacket {
                src_port,
                dst_port,
                __source_breaking,
            }) => Ok(Self::Udp {
                src_port: src_port.map(TryInto::try_into).transpose()?,
                dst_port: dst_port.map(TryInto::try_into).transpose()?,
            }),
            fnet_matchers::PacketTransportProtocol::Icmp(fnet_matchers::IcmpPacket {
                __source_breaking,
            }) => Ok(Self::Icmp),
            fnet_matchers::PacketTransportProtocol::Icmpv6(fnet_matchers::Icmpv6Packet {
                __source_breaking,
            }) => Ok(Self::Icmpv6),
            fnet_matchers::PacketTransportProtocol::__SourceBreaking { .. } => {
                Err(TransportProtocolError::UnknownUnionVariant)
            }
        }
    }
}

impl Debug for TransportProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Omit empty fields.
        match self {
            TransportProtocol::Tcp { src_port, dst_port } => {
                let mut debug_struct = f.debug_struct("Tcp");

                // Omit empty fields.
                if let Some(port) = &src_port {
                    let _ = debug_struct.field("src_port", port);
                }

                if let Some(port) = &dst_port {
                    let _ = debug_struct.field("dst_port", port);
                }

                debug_struct.finish()
            }
            TransportProtocol::Udp { src_port, dst_port } => {
                let mut debug_struct = f.debug_struct("Udp");

                // Omit empty fields.
                if let Some(port) = &src_port {
                    let _ = debug_struct.field("src_port", port);
                }

                if let Some(port) = &dst_port {
                    let _ = debug_struct.field("dst_port", port);
                }

                debug_struct.finish()
            }
            TransportProtocol::Icmp => f.write_str("Icmp"),
            TransportProtocol::Icmpv6 => f.write_str("Icmpv6"),
        }
    }
}

/// An extension type for [`fnet_matchers::Mark`]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Mark {
    Unmarked,
    Marked { mask: u32, between: RangeInclusive<u32>, invert: bool },
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum MarkError {
    #[error("mark union is of an unknown variant")]
    UnknownUnionVariant(u64),
}

impl TryFrom<fnet_matchers::Mark> for Mark {
    type Error = MarkError;

    fn try_from(matcher: fnet_matchers::Mark) -> Result<Self, Self::Error> {
        match matcher {
            fnet_matchers::Mark::Unmarked(fnet_matchers::Unmarked) => Ok(Mark::Unmarked),
            fnet_matchers::Mark::Marked(fnet_matchers::Marked {
                mask,
                between: fnet_matchers::Between { start, end },
                invert,
            }) => Ok(Mark::Marked { mask, between: RangeInclusive::new(start, end), invert }),
            fnet_matchers::Mark::__SourceBreaking { unknown_ordinal } => {
                Err(MarkError::UnknownUnionVariant(unknown_ordinal))
            }
        }
    }
}

impl From<Mark> for fnet_matchers::Mark {
    fn from(matcher: Mark) -> Self {
        match matcher {
            Mark::Unmarked => fnet_matchers::Mark::Unmarked(fnet_matchers::Unmarked),
            Mark::Marked { mask, between, invert } => {
                let (start, end) = between.into_inner();
                fnet_matchers::Mark::Marked(fnet_matchers::Marked {
                    mask,
                    between: fnet_matchers::Between { start, end },
                    invert,
                })
            }
        }
    }
}

/// An extension type for [`fnet_matchers::MarkInDomain`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MarkInDomain {
    pub domain: fnet::MarkDomain,
    pub mark: Mark,
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum MarkInDomainError {
    #[error("mark conversion failed: {0}")]
    Mark(MarkError),
}

impl TryFrom<fnet_matchers::MarkInDomain> for MarkInDomain {
    type Error = MarkInDomainError;

    fn try_from(matcher: fnet_matchers::MarkInDomain) -> Result<Self, Self::Error> {
        let fnet_matchers::MarkInDomain { domain, mark } = matcher;
        Ok(Self { domain, mark: mark.try_into().map_err(MarkInDomainError::Mark)? })
    }
}

impl From<MarkInDomain> for fnet_matchers::MarkInDomain {
    fn from(matcher: MarkInDomain) -> Self {
        let MarkInDomain { domain, mark } = matcher;
        Self { domain, mark: mark.into() }
    }
}

/// An extension type for [`fnet_matchers::TcpSocket`]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TcpSocket {
    Empty,
    SrcPort(BoundPort),
    DstPort(BoundPort),
    States(fnet_matchers::TcpState),
}

#[derive(Debug, PartialEq, Eq, Error)]
pub enum TcpSocketError {
    #[error("bound port matcher conversion failed: {0}")]
    BoundPort(BoundPortError),
    #[error("tcp union is of an unknown variant")]
    UnknownUnionVariant(u64),
}

impl TryFrom<fnet_matchers::TcpSocket> for TcpSocket {
    type Error = TcpSocketError;

    fn try_from(matcher: fnet_matchers::TcpSocket) -> Result<Self, Self::Error> {
        match matcher {
            fnet_matchers::TcpSocket::Empty(fnet_matchers::Empty) => Ok(Self::Empty),
            fnet_matchers::TcpSocket::SrcPort(port) => {
                Ok(Self::SrcPort(port.try_into().map_err(|e| TcpSocketError::BoundPort(e))?))
            }
            fnet_matchers::TcpSocket::DstPort(port) => {
                Ok(Self::DstPort(port.try_into().map_err(|e| TcpSocketError::BoundPort(e))?))
            }
            fnet_matchers::TcpSocket::States(states) => Ok(Self::States(states)),
            fnet_matchers::TcpSocket::__SourceBreaking { unknown_ordinal } => {
                Err(TcpSocketError::UnknownUnionVariant(unknown_ordinal))
            }
        }
    }
}

impl From<TcpSocket> for fnet_matchers::TcpSocket {
    fn from(matcher: TcpSocket) -> Self {
        match matcher {
            TcpSocket::Empty => Self::Empty(fnet_matchers::Empty),
            TcpSocket::SrcPort(port) => Self::SrcPort(port.into()),
            TcpSocket::DstPort(port) => Self::DstPort(port.into()),
            TcpSocket::States(states) => Self::States(states),
        }
    }
}

/// An extension type for [`fnet_matchers::UdpSocket`]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UdpSocket {
    Empty,
    SrcPort(BoundPort),
    DstPort(BoundPort),
    States(fnet_matchers::UdpState),
}

#[derive(Debug, PartialEq, Eq, Error)]
pub enum UdpSocketError {
    #[error("bound port matcher conversion failed: {0}")]
    BoundPort(BoundPortError),
    #[error("udp union is of an unknown variant")]
    UnknownUnionVariant(u64),
}

impl TryFrom<fnet_matchers::UdpSocket> for UdpSocket {
    type Error = UdpSocketError;

    fn try_from(matcher: fnet_matchers::UdpSocket) -> Result<Self, Self::Error> {
        match matcher {
            fnet_matchers::UdpSocket::Empty(fnet_matchers::Empty) => Ok(Self::Empty),
            fnet_matchers::UdpSocket::SrcPort(port) => {
                Ok(Self::SrcPort(port.try_into().map_err(|e| UdpSocketError::BoundPort(e))?))
            }
            fnet_matchers::UdpSocket::DstPort(port) => {
                Ok(Self::DstPort(port.try_into().map_err(|e| UdpSocketError::BoundPort(e))?))
            }
            fnet_matchers::UdpSocket::States(states) => Ok(Self::States(states)),
            fnet_matchers::UdpSocket::__SourceBreaking { unknown_ordinal } => {
                Err(UdpSocketError::UnknownUnionVariant(unknown_ordinal))
            }
        }
    }
}

impl From<UdpSocket> for fnet_matchers::UdpSocket {
    fn from(matcher: UdpSocket) -> Self {
        match matcher {
            UdpSocket::Empty => Self::Empty(fnet_matchers::Empty),
            UdpSocket::SrcPort(port) => Self::SrcPort(port.into()),
            UdpSocket::DstPort(port) => Self::DstPort(port.into()),
            UdpSocket::States(states) => Self::States(states),
        }
    }
}

/// An extension type for [`fnet_matchers::SocketTransportProtocol`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketTransportProtocol {
    Tcp(TcpSocket),
    Udp(UdpSocket),
}

#[derive(Debug, PartialEq, Eq, Error)]
pub enum SocketTransportProtocolError {
    #[error("invalid tcp matcher: {0}")]
    Tcp(TcpSocketError),
    #[error("invalid udp matcher: {0}")]
    Udp(UdpSocketError),
    #[error("socket transport protocol union is of an unknown variant")]
    UnknownUnionVariant(u64),
}

impl TryFrom<fnet_matchers::SocketTransportProtocol> for SocketTransportProtocol {
    type Error = SocketTransportProtocolError;

    fn try_from(matcher: fnet_matchers::SocketTransportProtocol) -> Result<Self, Self::Error> {
        match matcher {
            fnet_matchers::SocketTransportProtocol::Tcp(tcp) => {
                Ok(Self::Tcp(tcp.try_into().map_err(|e| SocketTransportProtocolError::Tcp(e))?))
            }
            fnet_matchers::SocketTransportProtocol::Udp(udp) => {
                Ok(Self::Udp(udp.try_into().map_err(|e| SocketTransportProtocolError::Udp(e))?))
            }
            fnet_matchers::SocketTransportProtocol::__SourceBreaking { unknown_ordinal } => {
                Err(SocketTransportProtocolError::UnknownUnionVariant(unknown_ordinal))
            }
        }
    }
}

impl From<SocketTransportProtocol> for fnet_matchers::SocketTransportProtocol {
    fn from(matcher: SocketTransportProtocol) -> Self {
        match matcher {
            SocketTransportProtocol::Tcp(tcp) => Self::Tcp(tcp.into()),
            SocketTransportProtocol::Udp(udp) => Self::Udp(udp.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use net_declare::{fidl_ip, fidl_ip_v4, fidl_ip_v6, fidl_subnet, net_subnet_v4, net_subnet_v6};
    use test_case::test_case;

    use super::*;

    #[test_case(
        fnet_matchers::Interface::Id(1),
        Interface::Id(NonZeroU64::new(1).unwrap());
        "Interface"
    )]
    #[test_case(
        fnet_matchers::BoundInterface::Unbound(fnet_matchers::Empty),
        BoundInterface::Unbound;
        "BoundInterface Unbound"
    )]
    #[test_case(
        fnet_matchers::BoundInterface::Bound(fnet_matchers::Interface::Id(1)),
        BoundInterface::Bound(Interface::Id(NonZeroU64::new(1).unwrap()));
        "BoundInterface Bound"
    )]
    #[test_case(
        fnet_matchers::Mark::Unmarked(fnet_matchers::Unmarked),
        Mark::Unmarked;
        "Unmarked"
    )]
    #[test_case(
        fnet_matchers::Mark::Marked(fnet_matchers::Marked {
            mask: 0xFF,
            between: fnet_matchers::Between { start: 10, end: 20 },
            invert: true,
        }),
        Mark::Marked { mask: 0xFF, between: 10..=20, invert: true };
        "Marked"
    )]
    #[test_case(
        fnet_matchers::MarkInDomain {
            domain: fnet::MarkDomain::Mark1,
            mark: fnet_matchers::Mark::Unmarked(fnet_matchers::Unmarked),
        },
        MarkInDomain {
            domain: fnet::MarkDomain::Mark1,
            mark: Mark::Unmarked,
        };
        "MarkInDomain"
    )]
    #[test_case(
        fnet_matchers::AddressMatcherType::Subnet(fidl_subnet!("192.0.2.0/24")),
        AddressMatcherType::Subnet(Subnet(net_subnet_v4!("192.0.2.0/24").into()));
        "AddressMatcherTypeV4"
    )]
    #[test_case(
        fnet_matchers::AddressMatcherType::Subnet(fidl_subnet!("2001:db8::/64")),
        AddressMatcherType::Subnet(Subnet(net_subnet_v6!("2001:db8::/64").into()));
        "AddressMatcherTypeV6"
    )]
    #[test_case(
        fnet_matchers::Address {
            matcher: fnet_matchers::AddressMatcherType::Subnet(fidl_subnet!("2001:db8::/64")),
            invert: true,
        },
        Address {
            matcher: AddressMatcherType::Subnet(Subnet(net_subnet_v6!("2001:db8::/64").into())),
            invert: true,
        };
        "AddressV6"
    )]
    #[test_case(
        fnet_matchers::AddressRange {
            start: fidl_ip!("192.0.2.0"),
            end: fidl_ip!("192.0.2.1"),
        },
        AddressRange::V4(
            fidl_ip_v4!("192.0.2.0")..=fidl_ip_v4!("192.0.2.1"),
        );
        "AddressRangeV4"
    )]
    #[test_case(
        fnet_matchers::AddressRange {
            start: fidl_ip!("2001:db8::0"),
            end: fidl_ip!("2001:db8::8"),
        },
        AddressRange::V6(
            fidl_ip_v6!("2001:db8::0")..=fidl_ip_v6!("2001:db8::8"),
        );
        "AddressRangeV6"
    )]
    #[test_case(
        fnet_matchers::PacketTransportProtocol::Udp(fnet_matchers::UdpPacket {
            src_port: Some(fnet_matchers::Port { start: 1024, end: u16::MAX, invert: false }),
            dst_port: None,
            ..Default::default()
        }),
        TransportProtocol::Udp {
            src_port: Some(Port { range: 1024..=u16::MAX, invert: false }),
            dst_port: None,
        };
        "TransportProtocol"
    )]
    #[test_case(
        fnet_matchers::TcpSocket::Empty(fnet_matchers::Empty),
        TcpSocket::Empty;
        "TcpSocketEmpty"
    )]
    #[test_case(
        fnet_matchers::TcpSocket::SrcPort(
            fnet_matchers::BoundPort::Bound(fnet_matchers::Port { start: 1024, end: u16::MAX, invert: false })
        ),
        TcpSocket::SrcPort(BoundPort::Bound(Port { range: 1024..=u16::MAX, invert: false }));
        "TcpSocketSrcPort"
    )]
    #[test_case(
        fnet_matchers::TcpSocket::DstPort(
            fnet_matchers::BoundPort::Bound(fnet_matchers::Port { start: 80, end: 80, invert: true })
        ),
        TcpSocket::DstPort(BoundPort::Bound(Port { range: 80..=80, invert: true }));
        "TcpSocketDstPort"
    )]
    #[test_case(
        fnet_matchers::TcpSocket::States(fnet_matchers::TcpState::ESTABLISHED),
        TcpSocket::States(fnet_matchers::TcpState::ESTABLISHED);
        "TcpSocketStates"
    )]
    #[test_case(
        fnet_matchers::UdpSocket::Empty(fnet_matchers::Empty),
        UdpSocket::Empty;
        "UdpSocketEmpty"
    )]
    #[test_case(
        fnet_matchers::UdpSocket::SrcPort(
            fnet_matchers::BoundPort::Bound(fnet_matchers::Port { start: 1024, end: u16::MAX, invert: false })
        ),
        UdpSocket::SrcPort(BoundPort::Bound(Port { range: 1024..=u16::MAX, invert: false }));
        "UdpSocketSrcPort"
    )]
    #[test_case(
        fnet_matchers::UdpSocket::DstPort(
            fnet_matchers::BoundPort::Bound(fnet_matchers::Port { start: 53, end: 53, invert: true })
        ),
        UdpSocket::DstPort(BoundPort::Bound(Port { range: 53..=53, invert: true }));
        "UdpSocketDstPort"
    )]
    #[test_case(
        fnet_matchers::UdpSocket::States(fnet_matchers::UdpState::BOUND),
        UdpSocket::States(fnet_matchers::UdpState::BOUND);
        "UdpSocketStates"
    )]
    #[test_case(
        fnet_matchers::SocketTransportProtocol::Tcp(
            fnet_matchers::TcpSocket::SrcPort(
                fnet_matchers::BoundPort::Bound(fnet_matchers::Port { start: 123, end: 123, invert: false })
            )
        ),
        SocketTransportProtocol::Tcp(TcpSocket::SrcPort(BoundPort::Bound(Port { range: 123..=123, invert: false })));
        "SocketTransportProtocolTcp"
    )]
    #[test_case(
        fnet_matchers::SocketTransportProtocol::Udp(
            fnet_matchers::UdpSocket::SrcPort(
                fnet_matchers::BoundPort::Bound(fnet_matchers::Port { start: 123, end: 123, invert: false })
            )
        ),
        SocketTransportProtocol::Udp(UdpSocket::SrcPort(BoundPort::Bound(Port { range: 123..=123, invert: false })));
        "SocketTransportProtocolUdp"
    )]
    #[test_case(
        fnet_matchers::BoundPort::Unbound(fnet_matchers::Empty),
        BoundPort::Unbound;
        "BoundPort Unbound"
    )]
    #[test_case(
        fnet_matchers::BoundAddress::Unbound(fnet_matchers::Empty),
        BoundAddress::Unbound;
        "BoundAddress Unbound"
    )]
    #[test_case(
        fnet_matchers::TcpSocket::SrcPort(fnet_matchers::BoundPort::Unbound(fnet_matchers::Empty)),
        TcpSocket::SrcPort(BoundPort::Unbound);
        "TcpSocket SrcPort Unbound"
    )]
    #[test_case(
        fnet_matchers::TcpSocket::DstPort(fnet_matchers::BoundPort::Unbound(fnet_matchers::Empty)),
        TcpSocket::DstPort(BoundPort::Unbound);
        "TcpSocket DstPort Unbound"
    )]
    #[test_case(
        fnet_matchers::UdpSocket::SrcPort(fnet_matchers::BoundPort::Unbound(fnet_matchers::Empty)),
        UdpSocket::SrcPort(BoundPort::Unbound);
        "UdpSocket SrcPort Unbound"
    )]
    #[test_case(
        fnet_matchers::UdpSocket::DstPort(fnet_matchers::BoundPort::Unbound(fnet_matchers::Empty)),
        UdpSocket::DstPort(BoundPort::Unbound);
        "UdpSocket DstPort Unbound"
    )]
    fn convert_from_fidl_and_back<F, E>(fidl_type: F, local_type: E)
    where
        E: TryFrom<F> + Clone + Debug + PartialEq,
        <E as TryFrom<F>>::Error: Debug + PartialEq,
        F: From<E> + Clone + Debug + PartialEq,
    {
        assert_eq!(fidl_type.clone().try_into(), Ok(local_type.clone()));
        assert_eq!(<_ as Into<F>>::into(local_type), fidl_type.clone());
    }

    #[test_case(
        fnet_matchers::BoundInterface::__SourceBreaking { unknown_ordinal: 0 } =>
            Err(BoundInterfaceError::UnknownUnionVariant(0));
        "UnknownUnionVariant"
    )]
    #[test_case(
        fnet_matchers::BoundInterface::Bound(fnet_matchers::Interface::Id(0)) =>
            Err(BoundInterfaceError::Interface(InterfaceError::ZeroId));
        "InterfaceError"
    )]
    fn bound_interface_try_from_error(
        fidl: fnet_matchers::BoundInterface,
    ) -> Result<BoundInterface, BoundInterfaceError> {
        BoundInterface::try_from(fidl)
    }

    #[test_case(
        fnet_matchers::Mark::__SourceBreaking { unknown_ordinal: 0 } =>
            Err(MarkError::UnknownUnionVariant(0));
        "UnknownUnionVariant"
    )]
    fn mark_try_from_error(fidl: fnet_matchers::Mark) -> Result<Mark, MarkError> {
        Mark::try_from(fidl)
    }

    #[test_case(
        fnet_matchers::MarkInDomain {
            domain: fnet::MarkDomain::Mark1,
            mark: fnet_matchers::Mark::__SourceBreaking { unknown_ordinal: 0 },
        } => Err(MarkInDomainError::Mark(MarkError::UnknownUnionVariant(0)));
        "MarkInDomain Mark Error"
    )]
    fn mark_and_domain_try_from_error(
        fidl: fnet_matchers::MarkInDomain,
    ) -> Result<MarkInDomain, MarkInDomainError> {
        MarkInDomain::try_from(fidl)
    }

    #[test]
    fn address_matcher_type_try_from_unknown_variant() {
        assert_eq!(
            AddressMatcherType::try_from(fnet_matchers::AddressMatcherType::__SourceBreaking {
                unknown_ordinal: 0
            }),
            Err(AddressMatcherTypeError::UnknownUnionVariant)
        );
    }

    #[test]
    fn subnet_try_from_invalid() {
        assert_eq!(
            Subnet::try_from(fnet::Subnet { addr: fidl_ip!("192.0.2.1"), prefix_len: 33 }),
            Err(SubnetError::PrefixTooLong)
        );
        assert_eq!(Subnet::try_from(fidl_subnet!("192.0.2.1/24")), Err(SubnetError::HostBitsSet));
    }

    #[test]
    fn address_range_try_from_invalid() {
        assert_eq!(
            AddressRange::try_from(fnet_matchers::AddressRange {
                start: fidl_ip!("192.0.2.1"),
                end: fidl_ip!("192.0.2.0"),
            }),
            Err(AddressRangeError::Invalid)
        );
        assert_eq!(
            AddressRange::try_from(fnet_matchers::AddressRange {
                start: fidl_ip!("2001:db8::1"),
                end: fidl_ip!("2001:db8::"),
            }),
            Err(AddressRangeError::Invalid)
        );
    }

    #[test]
    fn address_range_try_from_family_mismatch() {
        assert_eq!(
            AddressRange::try_from(fnet_matchers::AddressRange {
                start: fidl_ip!("192.0.2.0"),
                end: fidl_ip!("2001:db8::"),
            }),
            Err(AddressRangeError::FamilyMismatch)
        );
    }

    #[test]
    fn port_matcher_try_from_invalid() {
        assert_eq!(
            Port::try_from(fnet_matchers::Port { start: 1, end: 0, invert: false }),
            Err(PortError::InvalidPortRange)
        );
    }

    #[test]
    fn transport_protocol_try_from_unknown_variant() {
        assert_eq!(
            TransportProtocol::try_from(fnet_matchers::PacketTransportProtocol::__SourceBreaking {
                unknown_ordinal: 0
            }),
            Err(TransportProtocolError::UnknownUnionVariant)
        );
    }

    #[test_case(
        fnet_matchers::TcpSocket::__SourceBreaking { unknown_ordinal: 100 } =>
            Err(TcpSocketError::UnknownUnionVariant(100));
        "TcpSocket UnknownUnionVariant"
    )]
    #[test_case(
        fnet_matchers::TcpSocket::SrcPort(fnet_matchers::BoundPort::Bound(fnet_matchers::Port {
            start: 1,
            end: 0,
            invert: false,
        })) => Err(TcpSocketError::BoundPort(BoundPortError::Port(PortError::InvalidPortRange)));
        "TcpSocket SrcPort Error"
    )]
    #[test_case(
        fnet_matchers::TcpSocket::DstPort(fnet_matchers::BoundPort::Bound(fnet_matchers::Port {
            start: 1,
            end: 0,
            invert: false,
        })) => Err(TcpSocketError::BoundPort(BoundPortError::Port(PortError::InvalidPortRange)));
        "TcpSocket DstPort Error"
    )]
    fn tcp_socket_try_from_error(
        fidl: fnet_matchers::TcpSocket,
    ) -> Result<TcpSocket, TcpSocketError> {
        TcpSocket::try_from(fidl)
    }

    #[test_case(
        fnet_matchers::UdpSocket::__SourceBreaking { unknown_ordinal: 100 } =>
            Err(UdpSocketError::UnknownUnionVariant(100));
        "UdpSocket UnknownUnionVariant"
    )]
    #[test_case(
        fnet_matchers::UdpSocket::SrcPort(fnet_matchers::BoundPort::Bound(fnet_matchers::Port {
            start: 1,
            end: 0,
            invert: false,
        })) => Err(UdpSocketError::BoundPort(BoundPortError::Port(PortError::InvalidPortRange)));
        "UdpSocket SrcPort Error"
    )]
    #[test_case(
        fnet_matchers::UdpSocket::DstPort(fnet_matchers::BoundPort::Bound(fnet_matchers::Port {
            start: 1,
            end: 0,
            invert: false,
        })) => Err(UdpSocketError::BoundPort(BoundPortError::Port(PortError::InvalidPortRange)));
        "UdpSocket DstPort Error"
    )]
    fn udp_socket_try_from_error(
        fidl: fnet_matchers::UdpSocket,
    ) -> Result<UdpSocket, UdpSocketError> {
        UdpSocket::try_from(fidl)
    }

    #[test_case(
        fnet_matchers::SocketTransportProtocol::__SourceBreaking {
            unknown_ordinal: 100
        } => Err(SocketTransportProtocolError::UnknownUnionVariant(100));
        "SocketTransportProtocol UnknownUnionVariant"
    )]
    #[test_case(
        fnet_matchers::SocketTransportProtocol::Tcp(
            fnet_matchers::TcpSocket::__SourceBreaking { unknown_ordinal: 100 }
        ) => Err(SocketTransportProtocolError::Tcp(TcpSocketError::UnknownUnionVariant(100)));
        "SocketTransportProtocol Tcp Error"
    )]
    #[test_case(
        fnet_matchers::SocketTransportProtocol::Udp(
            fnet_matchers::UdpSocket::__SourceBreaking { unknown_ordinal: 100 }
        ) => Err(SocketTransportProtocolError::Udp(UdpSocketError::UnknownUnionVariant(100)));
        "SocketTransportProtocol Udp Error"
    )]
    fn socket_transport_protocol_try_from_error(
        fidl: fnet_matchers::SocketTransportProtocol,
    ) -> Result<SocketTransportProtocol, SocketTransportProtocolError> {
        SocketTransportProtocol::try_from(fidl)
    }
}
