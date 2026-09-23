// SPDX-License-Identifier: MIT

use netlink_packet_utils::Parseable;

use crate::AddressFamily;

use super::buffer::NeighbourDiscoveryUserOptionMessageBuffer;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[non_exhaustive]
pub enum NeighbourDiscoveryIcmpV6Type {
    RouterSolicitation,
    RouterAdvertisement,
    NeighbourSolicitation,
    NeighbourAdvertisement,
    Redirect,
    Other { type_: u8, code: u8 },
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[non_exhaustive]
pub enum NeighbourDiscoveryIcmpV4Type {
    Other { type_: u8, code: u8 },
}

impl NeighbourDiscoveryIcmpV4Type {
    pub fn into_type_and_code(self) -> (u8, u8) {
        match self {
            NeighbourDiscoveryIcmpV4Type::Other { type_, code } => (type_, code),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[non_exhaustive]
pub enum NeighbourDiscoveryIcmpType {
    Inet(NeighbourDiscoveryIcmpV4Type),
    Inet6(NeighbourDiscoveryIcmpV6Type),
    Other { family: AddressFamily, type_: u8, code: u8 },
}

impl NeighbourDiscoveryIcmpType {
    pub fn family(&self) -> AddressFamily {
        match self {
            NeighbourDiscoveryIcmpType::Inet(_) => AddressFamily::Inet,
            NeighbourDiscoveryIcmpType::Inet6(_) => AddressFamily::Inet6,
            NeighbourDiscoveryIcmpType::Other { family, type_: _, code: _ } => *family,
        }
    }

    pub fn into_type_and_code(self) -> (u8, u8) {
        match self {
            NeighbourDiscoveryIcmpType::Inet(type_) => type_.into_type_and_code(),
            NeighbourDiscoveryIcmpType::Inet6(type_) => type_.into_type_and_code(),
            NeighbourDiscoveryIcmpType::Other { family: _, type_, code } => (type_, code),
        }
    }
}

impl NeighbourDiscoveryIcmpV6Type {
    /// Determines the `NeighbourDiscoveryIcmpV6Type` from a given ICMP `type_` and `code`,
    /// as defined in RFC 4443 (for IPv6).
    pub fn new(type_: u8, code: u8) -> Self {
        match (type_, code) {
            (133, 0) => NeighbourDiscoveryIcmpV6Type::RouterSolicitation,
            (134, 0) => NeighbourDiscoveryIcmpV6Type::RouterAdvertisement,
            (135, 0) => NeighbourDiscoveryIcmpV6Type::NeighbourSolicitation,
            (136, 0) => NeighbourDiscoveryIcmpV6Type::NeighbourAdvertisement,
            (137, 0) => NeighbourDiscoveryIcmpV6Type::Redirect,
            _ => NeighbourDiscoveryIcmpV6Type::Other { type_, code },
        }
    }

    /// Given a `NeighbourDiscoveryIcmpV6Type`, returns the ICMP `type_` and `code`.
    pub fn into_type_and_code(self) -> (u8, u8) {
        match self {
            NeighbourDiscoveryIcmpV6Type::RouterSolicitation => (133, 0),
            NeighbourDiscoveryIcmpV6Type::RouterAdvertisement => (134, 0),
            NeighbourDiscoveryIcmpV6Type::NeighbourSolicitation => (135, 0),
            NeighbourDiscoveryIcmpV6Type::NeighbourAdvertisement => (136, 0),
            NeighbourDiscoveryIcmpV6Type::Redirect => (137, 0),
            NeighbourDiscoveryIcmpV6Type::Other { type_, code } => (type_, code),
        }
    }
}

impl<T: AsRef<[u8]>> Parseable<NeighbourDiscoveryUserOptionMessageBuffer<T>>
    for NeighbourDiscoveryUserOptionHeader
{
    type Error = netlink_packet_utils::DecodeError;

    fn parse(
        buf: &NeighbourDiscoveryUserOptionMessageBuffer<T>,
    ) -> Result<Self, netlink_packet_utils::DecodeError> {
        let icmp_type = match AddressFamily::from(buf.address_family()) {
            AddressFamily::Inet => {
                NeighbourDiscoveryIcmpType::Inet(NeighbourDiscoveryIcmpV4Type::Other {
                    type_: buf.icmp_type(),
                    code: buf.icmp_code(),
                })
            }
            AddressFamily::Inet6 => NeighbourDiscoveryIcmpType::Inet6(
                NeighbourDiscoveryIcmpV6Type::new(buf.icmp_type(), buf.icmp_code()),
            ),
            family => NeighbourDiscoveryIcmpType::Other {
                family,
                type_: buf.icmp_type(),
                code: buf.icmp_code(),
            },
        };
        Ok(Self { interface_index: buf.interface_index(), icmp_type })
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub struct NeighbourDiscoveryUserOptionHeader {
    /// The index of this ND user option's interface.
    pub interface_index: u32,
    /// The ICMP message type associated with this ND user option. As defined
    /// in RFC 792 for ICMPv4, and RFC 4443 for ICMPv6.
    pub icmp_type: NeighbourDiscoveryIcmpType,
}

impl NeighbourDiscoveryUserOptionHeader {
    pub fn new(interface_index: u32, icmp_type: NeighbourDiscoveryIcmpType) -> Self {
        Self { interface_index, icmp_type }
    }
}
