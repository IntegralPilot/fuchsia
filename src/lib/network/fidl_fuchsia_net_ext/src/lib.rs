// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extensions for types in the `fidl_fuchsia_net` crate.

use std::convert::TryInto as _;
use std::fmt::Display;

use fidl_fuchsia_net_common as fidl;

use net_types::{Witness as _, ethernet, ip};
use paste::paste;

/// Extension trait to provides access to FIDL types.
pub trait NetTypesIpAddressExt: ip::IpAddress {
    /// The equivalent FIDL address type.
    type Fidl: IntoExt<fidl::IpAddress> + FromExt<Self>;
}

impl NetTypesIpAddressExt for ip::Ipv4Addr {
    type Fidl = fidl::Ipv4Address;
}

impl NetTypesIpAddressExt for ip::Ipv6Addr {
    type Fidl = fidl::Ipv6Address;
}

impl FromExt<ip::Ipv4Addr> for fidl::Ipv4Address {
    fn from_ext(f: ip::Ipv4Addr) -> fidl::Ipv4Address {
        fidl::Ipv4Address { addr: f.ipv4_bytes() }
    }
}

impl FromExt<fidl::Ipv4Address> for ip::Ipv4Addr {
    fn from_ext(f: fidl::Ipv4Address) -> ip::Ipv4Addr {
        let fidl::Ipv4Address { addr } = f;
        ip::Ipv4Addr::new(addr)
    }
}

impl FromExt<ip::Ipv6Addr> for fidl::Ipv6Address {
    fn from_ext(f: ip::Ipv6Addr) -> fidl::Ipv6Address {
        fidl::Ipv6Address { addr: f.ipv6_bytes() }
    }
}

impl FromExt<fidl::Ipv6Address> for ip::Ipv6Addr {
    fn from_ext(f: fidl::Ipv6Address) -> ip::Ipv6Addr {
        let fidl::Ipv6Address { addr } = f;
        ip::Ipv6Addr::from_bytes(addr)
    }
}

impl FromExt<ip::IpAddr> for fidl::IpAddress {
    fn from_ext(f: ip::IpAddr) -> fidl::IpAddress {
        match f {
            ip::IpAddr::V4(v4) => {
                <ip::Ipv4Addr as IntoExt<fidl::Ipv4Address>>::into_ext(v4).into_ext()
            }
            ip::IpAddr::V6(v6) => {
                <ip::Ipv6Addr as IntoExt<fidl::Ipv6Address>>::into_ext(v6).into_ext()
            }
        }
    }
}

impl FromExt<fidl::IpAddress> for ip::IpAddr {
    fn from_ext(f: fidl::IpAddress) -> Self {
        match f {
            fidl::IpAddress::Ipv4(v4) => ip::IpAddr::V4(v4.into_ext()),
            fidl::IpAddress::Ipv6(v6) => ip::IpAddr::V6(v6.into_ext()),
        }
    }
}

impl TryFromExt<fidl::Ipv4AddressWithPrefix> for ip::Subnet<ip::Ipv4Addr> {
    type Error = ip::SubnetError;
    fn try_from_ext(
        fidl::Ipv4AddressWithPrefix { addr, prefix_len }: fidl::Ipv4AddressWithPrefix,
    ) -> Result<ip::Subnet<ip::Ipv4Addr>, Self::Error> {
        ip::Subnet::new(addr.into_ext(), prefix_len)
    }
}

impl TryFromExt<fidl::Ipv6AddressWithPrefix> for ip::Subnet<ip::Ipv6Addr> {
    type Error = ip::SubnetError;
    fn try_from_ext(
        fidl::Ipv6AddressWithPrefix { addr, prefix_len }: fidl::Ipv6AddressWithPrefix,
    ) -> Result<ip::Subnet<ip::Ipv6Addr>, Self::Error> {
        ip::Subnet::new(addr.into_ext(), prefix_len)
    }
}

impl<A: ip::IpAddress> FromExt<ip::Subnet<A>> for fidl::Subnet {
    fn from_ext(subnet: ip::Subnet<A>) -> fidl::Subnet {
        let addr: ip::IpAddr = subnet.network().into();
        fidl::Subnet { addr: addr.into_ext(), prefix_len: subnet.prefix() }
    }
}

impl<A: ip::IpAddress> FromExt<ip::AddrSubnet<A>> for fidl::Subnet {
    fn from_ext(subnet: ip::AddrSubnet<A>) -> fidl::Subnet {
        let addr: ip::IpAddr = subnet.addr().get().into();
        fidl::Subnet { addr: addr.into_ext(), prefix_len: subnet.subnet().prefix() }
    }
}

impl FromExt<ip::AddrSubnetEither> for fidl::Subnet {
    fn from_ext(addr_subnet: ip::AddrSubnetEither) -> fidl::Subnet {
        match addr_subnet {
            ip::AddrSubnetEither::V4(addr_subnet) => addr_subnet.into_ext(),
            ip::AddrSubnetEither::V6(addr_subnet) => addr_subnet.into_ext(),
        }
    }
}

impl FromExt<ip::IpVersion> for fidl::IpVersion {
    fn from_ext(version: ip::IpVersion) -> fidl::IpVersion {
        match version {
            ip::IpVersion::V4 => fidl::IpVersion::V4,
            ip::IpVersion::V6 => fidl::IpVersion::V6,
        }
    }
}

impl FromExt<fidl::IpVersion> for ip::IpVersion {
    fn from_ext(version: fidl::IpVersion) -> ip::IpVersion {
        match version {
            fidl::IpVersion::V4 => ip::IpVersion::V4,
            fidl::IpVersion::V6 => ip::IpVersion::V6,
        }
    }
}

/// Extension trait to allow user-friendly formatting.
pub trait DisplayExt {
    type Displayable: Display;

    /// Returns a [`Display`]-able variant..
    fn display_ext(&self) -> Self::Displayable;
}

/// Extension to IP types.
pub trait IpExt {
    /// Is the address a unicast and link-local address?
    fn is_unicast_link_local(&self) -> bool;
}

impl IpExt for fidl::Ipv6Address {
    fn is_unicast_link_local(&self) -> bool {
        ip::Ipv6Addr::from_bytes(self.addr).is_unicast_link_local()
    }
}

/// A manual implementation of `From`.
pub trait FromExt<T> {
    /// Performs the conversion.
    fn from_ext(f: T) -> Self;
}

/// A manual implementation of `Into`.
///
/// A blanket implementation is provided for implementers of `FromExt<T>`.
pub trait IntoExt<T> {
    /// Performs the conversion.
    fn into_ext(self) -> T;
}

impl<T, U> IntoExt<U> for T
where
    U: FromExt<T>,
{
    fn into_ext(self) -> U {
        U::from_ext(self)
    }
}

/// A manual implementation of `TryFrom`.
pub trait TryFromExt<T>: Sized {
    type Error;
    /// Tries to perform the conversion.
    fn try_from_ext(f: T) -> Result<Self, Self::Error>;
}

/// A manual implementation of `TryInto`.
///
/// A blanket implementation is provided for implementers of `TryFromExt<T>`.
pub trait TryIntoExt<T>: Sized {
    type Error;
    /// Tries to perform the conversion.
    fn try_into_ext(self) -> Result<T, Self::Error>;
}

impl<T, U> TryIntoExt<U> for T
where
    U: TryFromExt<T>,
{
    type Error = U::Error;
    fn try_into_ext(self) -> Result<U, Self::Error> {
        U::try_from_ext(self)
    }
}

impl FromExt<fidl::Ipv4Address> for fidl::IpAddress {
    fn from_ext(f: fidl::Ipv4Address) -> fidl::IpAddress {
        fidl::IpAddress::Ipv4(f)
    }
}

impl FromExt<fidl::Ipv6Address> for fidl::IpAddress {
    fn from_ext(f: fidl::Ipv6Address) -> fidl::IpAddress {
        fidl::IpAddress::Ipv6(f)
    }
}

impl FromExt<fidl::Ipv4AddressWithPrefix> for fidl::Subnet {
    fn from_ext(
        fidl::Ipv4AddressWithPrefix { addr, prefix_len }: fidl::Ipv4AddressWithPrefix,
    ) -> fidl::Subnet {
        fidl::Subnet { addr: addr.into_ext(), prefix_len }
    }
}

impl FromExt<fidl::Ipv6AddressWithPrefix> for fidl::Subnet {
    fn from_ext(
        fidl::Ipv6AddressWithPrefix { addr, prefix_len }: fidl::Ipv6AddressWithPrefix,
    ) -> fidl::Subnet {
        fidl::Subnet { addr: addr.into_ext(), prefix_len }
    }
}

impl FromExt<fidl::Ipv4SocketAddress> for fidl::SocketAddress {
    fn from_ext(f: fidl::Ipv4SocketAddress) -> fidl::SocketAddress {
        fidl::SocketAddress::Ipv4(f)
    }
}

impl FromExt<fidl::Ipv6SocketAddress> for fidl::SocketAddress {
    fn from_ext(f: fidl::Ipv6SocketAddress) -> fidl::SocketAddress {
        fidl::SocketAddress::Ipv6(f)
    }
}

impl FromExt<fidl::MacAddress> for ethernet::Mac {
    fn from_ext(fidl::MacAddress { octets }: fidl::MacAddress) -> Self {
        ethernet::Mac::new(octets)
    }
}

impl FromExt<ethernet::Mac> for fidl::MacAddress {
    fn from_ext(mac: ethernet::Mac) -> fidl::MacAddress {
        fidl::MacAddress { octets: mac.bytes() }
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub struct IpAddress(pub std::net::IpAddr);

impl std::fmt::Display for IpAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        let IpAddress(ip_address) = self;
        write!(f, "{}", ip_address)
    }
}

impl From<fidl::IpAddress> for IpAddress {
    fn from(addr: fidl::IpAddress) -> IpAddress {
        IpAddress(match addr {
            fidl::IpAddress::Ipv4(fidl::Ipv4Address { addr }) => addr.into(),
            fidl::IpAddress::Ipv6(fidl::Ipv6Address { addr }) => addr.into(),
        })
    }
}

impl From<IpAddress> for fidl::IpAddress {
    fn from(IpAddress(ip_address): IpAddress) -> Self {
        match ip_address {
            std::net::IpAddr::V4(v4addr) => {
                fidl::IpAddress::Ipv4(fidl::Ipv4Address { addr: v4addr.octets() })
            }
            std::net::IpAddr::V6(v6addr) => {
                fidl::IpAddress::Ipv6(fidl::Ipv6Address { addr: v6addr.octets() })
            }
        }
    }
}

impl std::str::FromStr for IpAddress {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(IpAddress(s.parse()?))
    }
}

macro_rules! generate_address_type {
    ( $ip:ident ) => {
        paste! {
            #[derive(PartialEq, Eq, Debug, Clone, Copy)]
            pub struct [<Ip $ip Address>](pub std::net::[<Ip $ip Addr>]);

            impl std::fmt::Display for [<Ip $ip Address>] {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
                    let Self(addr) = self;
                    write!(f, "{}", addr)
                }
            }

            impl From<fidl::[<Ip $ip Address>]> for [<Ip $ip Address>] {
                fn from(fidl::[<Ip $ip Address>] { addr }: fidl::[<Ip $ip Address>]) -> Self {
                    Self(addr.into())
                }
            }

            impl From<[<Ip $ip Address>]> for fidl::[<Ip $ip Address>] {
                fn from([<Ip $ip Address>](addr): [<Ip $ip Address>]) -> Self {
                    Self { addr: addr.octets() }
                }
            }

            impl std::str::FromStr for [<Ip $ip Address>] {
                type Err = std::net::AddrParseError;
                fn from_str(s: &str) -> Result<Self, Self::Err> {
                    Ok(Self(s.parse()?))
                }
            }
        }
    };
}
generate_address_type!(v4);
generate_address_type!(v6);

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub struct Subnet {
    pub addr: IpAddress,
    pub prefix_len: u8,
}

impl std::fmt::Display for Subnet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        let Self { addr, prefix_len } = self;
        write!(f, "{}/{}", addr, prefix_len)
    }
}

impl std::str::FromStr for Subnet {
    type Err = anyhow::Error;

    // Parse a Subnet from a CIDR-notated IP address.
    //
    // NB: if we need additional CIDR related functionality in the future,
    // we should consider pulling in https://crates.io/crates/cidr
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut pieces = s.split('/');
        let addr = pieces
            .next()
            .expect("String#split should never return an empty iterator")
            .parse::<std::net::IpAddr>()?;

        let addr_len = match addr {
            std::net::IpAddr::V4(_) => 32,
            std::net::IpAddr::V6(_) => 128,
        };
        let validated_prefix = match pieces.next() {
            Some(p) => {
                let parsed_len = p.parse::<u8>()?;
                if parsed_len > addr_len {
                    Err(anyhow::format_err!(
                        "prefix length provided ({} bits) too large. address {} is only {} bits long",
                        parsed_len,
                        addr,
                        addr_len
                    ))
                } else {
                    Ok(parsed_len)
                }
            }
            None => Ok(addr_len),
        };

        match pieces.next() {
            Some(_) => Err(anyhow::format_err!(
                "more than one '/' separator found while attempting to parse CIDR string {}",
                s
            )),
            None => Ok(()),
        }?;
        let addr = IpAddress(addr);
        Ok(Subnet { addr, prefix_len: validated_prefix? })
    }
}

impl From<fidl::Subnet> for Subnet {
    fn from(subnet: fidl::Subnet) -> Self {
        let fidl::Subnet { addr, prefix_len } = subnet;
        let addr = addr.into();
        Self { addr, prefix_len }
    }
}

impl From<Subnet> for fidl::Subnet {
    fn from(subnet: Subnet) -> fidl::Subnet {
        let Subnet { addr, prefix_len } = subnet;
        let addr = addr.into();
        fidl::Subnet { addr, prefix_len }
    }
}

/// Returns a subnet which guarantees the masked bits on its IP address are
/// zero.
pub fn apply_subnet_mask(subnet: fidl::Subnet) -> fidl::Subnet {
    let fidl::Subnet { addr, prefix_len } = subnet;
    use net_types::ip::IpAddress as _;
    let addr = match addr {
        fidl::IpAddress::Ipv4(fidl::Ipv4Address { addr }) => {
            let addr = net_types::ip::Ipv4Addr::from(addr).mask(prefix_len).ipv4_bytes();
            fidl::IpAddress::Ipv4(fidl::Ipv4Address { addr })
        }
        fidl::IpAddress::Ipv6(fidl::Ipv6Address { addr }) => {
            let addr = net_types::ip::Ipv6Addr::from(addr).mask(prefix_len).ipv6_bytes();
            fidl::IpAddress::Ipv6(fidl::Ipv6Address { addr })
        }
    };
    fidl::Subnet { addr, prefix_len }
}

macro_rules! generate_subnet_type {
    ( $ip:ident, $prefix_len:literal ) => {
        paste! {
            #[derive(PartialEq, Eq, Debug, Clone, Copy)]
            pub struct [<Subnet $ip:upper>] {
                pub addr: [<Ip $ip Address>],
                pub prefix_len: u8,
            }

            impl std::fmt::Display for [<Subnet $ip:upper>] {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
                    let Self { addr, prefix_len } = self;
                    write!(f, "{}/{}", addr, prefix_len)
                }
            }

            impl std::str::FromStr for [<Subnet $ip:upper>] {
                type Err = anyhow::Error;

                // Parse a Subnet from a CIDR-notated IP address.
                //
                // NB: if we need additional CIDR related functionality in the future,
                // we should consider pulling in https://crates.io/crates/cidr
                fn from_str(s: &str) -> Result<Self, Self::Err> {
                    let mut pieces = s.split('/');
                    let addr = pieces
                        .next()
                        .expect("String#split should never return an empty iterator")
                        .parse::<std::net::[<Ip $ip Addr>]>()?;

                    let addr_len = $prefix_len;
                    let validated_prefix = match pieces.next() {
                        Some(p) => {
                            let parsed_len = p.parse::<u8>()?;
                            if parsed_len > addr_len {
                                Err(anyhow::format_err!(
                                    "prefix length provided ({} bits) too large. address {} is only {} bits long",
                                    parsed_len,
                                    addr,
                                    addr_len
                                ))
                            } else {
                                Ok(parsed_len)
                            }
                        }
                        None => Ok(addr_len),
                    };

                    match pieces.next() {
                        Some(_) => Err(anyhow::format_err!(
                            "more than one '/' separator found while attempting to parse CIDR string {}",
                            s
                        )),
                        None => Ok(()),
                    }?;
                    let addr = [<Ip $ip Address>](addr);
                    Ok([<Subnet $ip:upper>] { addr, prefix_len: validated_prefix? })
                }
            }

            impl From<fidl::[<Ip $ip AddressWithPrefix>]> for [<Subnet $ip:upper>] {
                fn from(
                    fidl::[<Ip $ip AddressWithPrefix>] { addr, prefix_len }:
                        fidl::[<Ip $ip AddressWithPrefix>]
                ) -> Self {
                    Self { addr: addr.into(), prefix_len }
                }
            }

            impl From<[<Subnet $ip:upper>]> for fidl::[<Ip $ip AddressWithPrefix>] {
                fn from([<Subnet $ip:upper>] { addr, prefix_len }: [<Subnet $ip:upper>]) -> Self {
                    Self { addr: addr.into(), prefix_len }
                }
            }
        }
    };
}
generate_subnet_type!(v4, 32);
generate_subnet_type!(v6, 128);

#[derive(PartialEq, Eq, Clone, Copy, Hash)]
pub struct MacAddress {
    pub octets: [u8; 6],
}

impl From<fidl::MacAddress> for MacAddress {
    fn from(fidl::MacAddress { octets }: fidl::MacAddress) -> Self {
        Self { octets }
    }
}

impl From<MacAddress> for fidl::MacAddress {
    fn from(MacAddress { octets }: MacAddress) -> fidl::MacAddress {
        fidl::MacAddress { octets }
    }
}

// We deliberately redirect Debug to use Display so that the MacAddress
// will be printed using hex formatting.
impl std::fmt::Debug for MacAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

impl std::fmt::Display for MacAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { octets } = self;
        for (i, byte) in octets.iter().enumerate() {
            if i > 0 {
                write!(f, ":")?;
            }
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

impl serde::Serialize for MacAddress {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self)
    }
}

impl<'de> serde::Deserialize<'de> for MacAddress {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        <Self as std::str::FromStr>::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl std::str::FromStr for MacAddress {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use anyhow::Context;

        let mut octets = [0; 6];
        let mut iter = s.split(':');
        for (i, octet) in octets.iter_mut().enumerate() {
            let next_octet = iter.next().ok_or_else(|| {
                anyhow::format_err!("MAC address [{}] only specifies {} out of 6 octets", s, i)
            })?;
            *octet = u8::from_str_radix(next_octet, 16)
                .with_context(|| format!("could not parse hex integer from {}", next_octet))?;
        }
        if iter.next().is_some() {
            return Err(anyhow::format_err!("MAC address has more than six octets: {}", s));
        }
        Ok(MacAddress { octets })
    }
}

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub struct SocketAddress(pub std::net::SocketAddr);

impl std::fmt::Display for SocketAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(socket_addr) = self;
        std::fmt::Display::fmt(socket_addr, f)
    }
}

impl DisplayExt for fidl::SocketAddress {
    type Displayable = SocketAddress;
    fn display_ext(&self) -> SocketAddress {
        self.clone().into()
    }
}

impl<T: IntoExt<fidl::SocketAddress> + Clone> DisplayExt for T {
    type Displayable = SocketAddress;
    fn display_ext(&self) -> SocketAddress {
        IntoExt::into_ext(self.clone()).into()
    }
}

impl From<fidl::SocketAddress> for SocketAddress {
    fn from(f: fidl::SocketAddress) -> Self {
        Self(match f {
            fidl::SocketAddress::Ipv4(fidl::Ipv4SocketAddress {
                address: fidl::Ipv4Address { addr },
                port,
            }) => std::net::SocketAddr::V4(std::net::SocketAddrV4::new(addr.into(), port)),
            fidl::SocketAddress::Ipv6(fidl::Ipv6SocketAddress {
                address: fidl::Ipv6Address { addr },
                port,
                zone_index,
            }) => std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
                addr.into(),
                port,
                0,
                zone_index.try_into().unwrap_or(0),
            )),
        })
    }
}

impl From<SocketAddress> for fidl::SocketAddress {
    fn from(SocketAddress(addr): SocketAddress) -> Self {
        match addr {
            std::net::SocketAddr::V4(socket_addr) => {
                fidl::SocketAddress::Ipv4(fidl::Ipv4SocketAddress {
                    address: fidl::Ipv4Address { addr: socket_addr.ip().octets() },
                    port: socket_addr.port(),
                })
            }
            std::net::SocketAddr::V6(socket_addr) => {
                fidl::SocketAddress::Ipv6(fidl::Ipv6SocketAddress {
                    address: fidl::Ipv6Address { addr: socket_addr.ip().octets() },
                    port: socket_addr.port(),
                    zone_index: socket_addr.scope_id().into(),
                })
            }
        }
    }
}

/// This struct represents all the marks the netstack supports.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Marks {
    /// The mark for the MARK_1 domain.
    pub mark_1: Option<u32>,
    /// The mark for the MARK_2 domain.
    pub mark_2: Option<u32>,
}

/// The maximum number of mark domains supported.
pub const MAX_MARK_DOMAIN_COUNTS: usize = 2;

impl Marks {
    pub fn get(&self, domain: fidl::MarkDomain) -> Option<u32> {
        match domain {
            fidl::MarkDomain::Mark1 => self.mark_1,
            fidl::MarkDomain::Mark2 => self.mark_2,
        }
    }
}

impl IntoIterator for Marks {
    type Item = (fidl::MarkDomain, fidl::Mark);

    type IntoIter = std::iter::Flatten<
        std::array::IntoIter<Option<(fidl::MarkDomain, fidl::Mark)>, MAX_MARK_DOMAIN_COUNTS>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        let Self { mark_1, mark_2 } = self;
        [
            mark_1.map(|mark| (fidl::MarkDomain::Mark1, mark)),
            mark_2.map(|mark| (fidl::MarkDomain::Mark2, mark)),
        ]
        .into_iter()
        .flatten()
    }
}

impl From<fidl::Marks> for Marks {
    fn from(value: fidl::Marks) -> Self {
        let fidl::Marks { mark_1, mark_2, __source_breaking } = value;
        return Self { mark_1, mark_2 };
    }
}

impl From<Marks> for fidl::Marks {
    fn from(value: Marks) -> Self {
        let Marks { mark_1, mark_2 } = value;
        Self { mark_1, mark_2, __source_breaking: Default::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use net_declare::{
        fidl_ip_v4_with_prefix, fidl_ip_v6_with_prefix, fidl_subnet, net_addr_subnet,
        net_addr_subnet_v4, net_addr_subnet_v6, net_subnet_v4, net_subnet_v6, std_ip, std_ip_v4,
        std_ip_v6,
    };
    use test_case::test_case;

    use std::collections::HashMap;
    use std::str::FromStr;

    #[test]
    fn test_from_into_ext() {
        let a = fidl::Ipv4Address { addr: [0; 4] };
        assert_eq!(fidl::IpAddress::Ipv4(a), a.into_ext());

        let a = fidl::Ipv4Address { addr: [0; 4] };
        assert_eq!(net_types::ip::Ipv4Addr::new([0; 4]), a.into_ext());

        let a = fidl::Ipv6Address { addr: [0; 16] };
        assert_eq!(fidl::IpAddress::Ipv6(a), a.into_ext());

        let a = fidl::Ipv6Address { addr: [0; 16] };
        assert_eq!(net_types::ip::Ipv6Addr::from_bytes([0; 16]), a.into_ext());

        let a = fidl::Ipv4SocketAddress { address: fidl::Ipv4Address { addr: [0; 4] }, port: 1 };
        assert_eq!(fidl::SocketAddress::Ipv4(a), a.into_ext());

        let a = fidl::Ipv6SocketAddress {
            address: fidl::Ipv6Address { addr: [0; 16] },
            port: 1,
            zone_index: 2,
        };
        assert_eq!(fidl::SocketAddress::Ipv6(a), a.into_ext());
    }

    #[test]
    fn test_ipaddr() {
        let want_ext = IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::new(1, 2, 3, 4)));
        let want_fidl = fidl::IpAddress::Ipv4(fidl::Ipv4Address { addr: [1, 2, 3, 4] });
        let got_fidl: fidl::IpAddress = want_ext.into();
        let got_ext = IpAddress::from(got_fidl);

        assert_eq!(want_ext, got_ext);
        assert_eq!(want_fidl, got_fidl);
    }

    #[test]
    fn test_net_types_subnet_into_fidl_subnet() {
        assert_eq!(fidl_subnet!("192.168.0.0/24"), net_subnet_v4!("192.168.0.0/24").into_ext());
        assert_eq!(fidl_subnet!("fd::/64"), net_subnet_v6!("fd::/64").into_ext());
    }

    // Note "1.2.3.4" or "::" is a valid form. Subnet's FromStr trait allows
    // missing prefix, and assumes the legally maximum prefix length.
    macro_rules! subnet_from_str_invalid {
        ($typ:ty) => {
            paste! {
                #[test_case("")]
                #[test_case("/32")]                                   // no ip address
                #[test_case(" /32"; "space_slash_32")]                // no ip address
                #[test_case("192.0.2.0/8/8")]                         // too many slashes
                #[test_case("192.0.2.0/33")]                          // prefix too long
                #[test_case("192.0.2.0:8080")]                        // that's a port, not a prefix
                #[test_case("2001:db8::e1bf:4fe9:fb62:e3f4/129")]     // prefix too long
                #[test_case("2001:db8::e1bf:4fe9:fb62:e3f4/32%eth0")] // zone index
                fn [<$typ:snake _from_str_invalid>](s: &str) {
                    let _: anyhow::Error = $typ::from_str(s).expect_err(&format!(
                        "a malformed str is wrongfully convertitable to Subnet struct: \"{}\"",
                        s
                    ));
                }
            }
        };
    }

    subnet_from_str_invalid!(Subnet);
    subnet_from_str_invalid!(SubnetV4);
    subnet_from_str_invalid!(SubnetV6);

    #[test_case(
        "192.0.2.0/24",
        Subnet { addr: IpAddress(std_ip!("192.0.2.0")), prefix_len: 24 },
        fidl_subnet!("192.0.2.0/24")
    )]
    #[test_case(
        "2001:db8::/32",
        Subnet { addr: IpAddress(std_ip!("2001:db8::")), prefix_len: 32 },
        fidl_subnet!("2001:db8::/32")
    )]
    fn subnet_conversions(want_str: &str, want_ext: Subnet, want_fidl: fidl::Subnet) {
        let got_ext = Subnet::from_str(want_str).ok().expect("conversion error");
        let got_fidl: fidl::Subnet = got_ext.into();
        let got_ext_back = Subnet::from(got_fidl);
        let got_str = &format!("{}", got_ext_back);

        assert_eq!(want_ext, got_ext);
        assert_eq!(want_fidl, got_fidl);
        assert_eq!(got_ext, got_ext_back);
        assert_eq!(want_str, got_str);
    }

    #[test]
    fn subnet_v4_from_str() {
        assert_eq!(
            SubnetV4::from_str("192.0.2.0/24")
                .expect("should be able to parse 192.0.2.0/24 into SubnetV4"),
            SubnetV4 { addr: Ipv4Address(std_ip_v4!("192.0.2.0")), prefix_len: 24 }
        );
    }

    #[test]
    fn subnet_v4_from_v6_str() {
        let _: anyhow::Error = SubnetV4::from_str("2001:db8::/24")
            .expect_err("IPv6 subnet should not be parsed as SubnetV4 successfully");
    }

    #[test]
    fn subnet_v6_from_str() {
        assert_eq!(
            SubnetV6::from_str("2001:db8::/32")
                .expect("should be able to parse 2001:db8::/32 into SubnetV6"),
            SubnetV6 { addr: Ipv6Address(std_ip_v6!("2001:db8::")), prefix_len: 32 }
        );
    }

    #[test]
    fn subnet_v6_from_v4_str() {
        let _: anyhow::Error = SubnetV6::from_str("192.0.2.0/24")
            .expect_err("IPv4 subnet should not be parsed as SubnetV6 successfully");
    }

    #[test]
    fn test_subnet_try_from_ipaddress_with_prefix() {
        // Test Success
        assert_eq!(
            Ok(net_subnet_v4!("192.168.0.0/24")),
            fidl_ip_v4_with_prefix!("192.168.0.0/24").try_into_ext()
        );
        assert_eq!(
            Ok(net_subnet_v6!("fe80::/64")),
            fidl_ip_v6_with_prefix!("fe80::/64").try_into_ext()
        );

        // Test Failure (Host Bits Set)
        assert_matches!(
            ip::Subnet::<ip::Ipv4Addr>::try_from_ext(fidl_ip_v4_with_prefix!("192.168.0.1/24")),
            Err(ip::SubnetError::HostBitsSet)
        );
        assert_matches!(
            ip::Subnet::<ip::Ipv6Addr>::try_from_ext(fidl_ip_v6_with_prefix!("fe80::1/64")),
            Err(ip::SubnetError::HostBitsSet)
        );
    }

    #[test]
    fn test_addr_subnet_fidl_subnet() {
        assert_eq!(
            fidl::Subnet::from_ext(net_addr_subnet_v4!("192.168.0.8/24")),
            fidl_subnet!("192.168.0.8/24")
        );

        assert_eq!(
            fidl::Subnet::from_ext(net_addr_subnet_v6!("fe80::1234/54")),
            fidl_subnet!("fe80::1234/54")
        );
    }

    #[test]
    fn test_addr_subnet_either_fidl_subnet() {
        assert_eq!(
            fidl::Subnet::from_ext(net_addr_subnet!("192.168.0.8/24")),
            fidl_subnet!("192.168.0.8/24")
        );
        assert_eq!(
            fidl::Subnet::from_ext(net_addr_subnet!("fe80::1234/54")),
            fidl_subnet!("fe80::1234/54")
        );
    }

    #[test]
    fn mac_addr_from_str_with_valid_str_returns_mac_addr() {
        let result = MacAddress::from_str("AA:BB:CC:DD:EE:FF").unwrap();
        let expected = MacAddress { octets: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF] };

        assert_eq!(expected, result);
    }

    #[test]
    fn mac_addr_from_str_with_invalid_digit_returns_err() {
        let result = MacAddress::from_str("11:22:33:44:55:GG");

        assert!(result.is_err());
    }

    #[test]
    fn mac_addr_from_str_with_invalid_format_returns_err() {
        let result = MacAddress::from_str("11-22-33-44-55-66");

        assert!(result.is_err());
    }

    #[test]
    fn mac_addr_from_str_with_empty_string_returns_err() {
        let result = MacAddress::from_str("");

        assert!(result.is_err());
    }

    #[test]
    fn mac_addr_from_str_with_extra_quotes_returns_err() {
        let result = MacAddress::from_str("\"11:22:33:44:55:66\"");

        assert!(result.is_err());
    }

    #[test]
    fn valid_mac_addr_array_deserializes_to_vec_of_mac_addrs() {
        let result: Vec<MacAddress> =
            serde_json::from_str("[\"11:11:11:11:11:11\", \"AA:AA:AA:AA:AA:AA\"]").unwrap();
        let expected = vec![
            MacAddress { octets: [0x11, 0x11, 0x11, 0x11, 0x11, 0x11] },
            MacAddress { octets: [0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA] },
        ];

        assert_eq!(expected, result);
    }

    #[test]
    fn mac_addr_to_mac_addr_map_deserializes_to_hashmap() {
        let result: HashMap<MacAddress, MacAddress> =
            serde_json::from_str("{\"11:22:33:44:55:66\": \"AA:BB:CC:DD:EE:FF\"}").unwrap();
        let expected: HashMap<_, _> = std::iter::once((
            MacAddress { octets: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66] },
            MacAddress { octets: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF] },
        ))
        .collect();

        assert_eq!(expected, result);
    }

    #[test]
    fn mac_addr_to_mac_addr_map_serializes_to_valid_json() {
        let mac_addr_map: HashMap<_, _> = std::iter::once((
            MacAddress { octets: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66] },
            MacAddress { octets: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF] },
        ))
        .collect();

        let result = serde_json::to_string(&mac_addr_map).unwrap();

        assert_eq!("{\"11:22:33:44:55:66\":\"aa:bb:cc:dd:ee:ff\"}", result);
    }

    #[test]
    fn test_socket_addr() {
        // V4.
        let want_ext = SocketAddress(std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
            std::net::Ipv4Addr::new(1, 2, 3, 4),
            5,
        )));
        let want_fidl = fidl::SocketAddress::Ipv4(fidl::Ipv4SocketAddress {
            address: fidl::Ipv4Address { addr: [1, 2, 3, 4] },
            port: 5,
        });
        let got_fidl: fidl::SocketAddress = want_ext.into();
        let got_ext = SocketAddress::from(want_fidl);

        assert_eq!(want_ext, got_ext);
        assert_eq!(want_fidl, got_fidl);

        // V6.
        let want_ext = SocketAddress(std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
            std::net::Ipv6Addr::new(0x0102, 0x0304, 0x0506, 0x0708, 0x090A, 0x0B0C, 0x0D0E, 0x0F10),
            17,
            0,
            18,
        )));
        let want_fidl = fidl::SocketAddress::Ipv6(fidl::Ipv6SocketAddress {
            address: fidl::Ipv6Address {
                addr: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
            },
            port: 17,
            zone_index: 18,
        });
        let got_fidl: fidl::SocketAddress = want_ext.into();
        let got_ext = SocketAddress::from(got_fidl);

        assert_eq!(want_ext, got_ext);
        assert_eq!(want_fidl, want_fidl);
    }

    #[test]
    fn test_display_ext() {
        let ipv4_sock_addr =
            fidl::Ipv4SocketAddress { address: fidl::Ipv4Address { addr: [1, 2, 3, 4] }, port: 5 };
        assert_eq!(
            SocketAddress(std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::new(1, 2, 3, 4),
                5,
            ))),
            fidl::SocketAddress::Ipv4(ipv4_sock_addr).display_ext()
        );
        assert_eq!(
            SocketAddress(std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::new(1, 2, 3, 4),
                5,
            ))),
            ipv4_sock_addr.display_ext()
        );
        assert_eq!(
            SocketAddress(std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
                std::net::Ipv6Addr::new(
                    0x0102, 0x0304, 0x0506, 0x0708, 0x090A, 0x0B0C, 0x0D0E, 0x0F10
                ),
                17,
                0,
                18,
            ))),
            fidl::Ipv6SocketAddress {
                address: fidl::Ipv6Address {
                    addr: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
                },
                port: 17,
                zone_index: 18,
            }
            .display_ext()
        );
    }
}
