// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::{Debug, Display};
use std::marker::PhantomData;
use std::net::{Ipv4Addr, Ipv6Addr};

use netext::IsLocalAddr;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// IP address with support for IPv6 scope identifiers as defined in RFC 4007.
#[derive(Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub enum IpAddr {
    /// An IPv4 address.
    V4(Ipv4Addr),
    /// An IPv6 address with optional scope identifier.
    V6(Ipv6Addr, Option<String>),
}

impl Into<std::net::IpAddr> for IpAddr {
    fn into(self) -> std::net::IpAddr {
        match self {
            IpAddr::V4(ip) => std::net::IpAddr::from(ip),
            IpAddr::V6(ip, _) => std::net::IpAddr::from(ip),
        }
    }
}

impl From<Ipv6Addr> for IpAddr {
    fn from(value: Ipv6Addr) -> Self {
        IpAddr::V6(value, None)
    }
}

impl From<Ipv4Addr> for IpAddr {
    fn from(value: Ipv4Addr) -> Self {
        IpAddr::V4(value)
    }
}

impl From<std::net::IpAddr> for IpAddr {
    fn from(value: std::net::IpAddr) -> Self {
        match value {
            std::net::IpAddr::V4(ip) => IpAddr::from(ip),
            std::net::IpAddr::V6(ip) => IpAddr::from(ip),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
/// An error which can be returned when parsing an IP address with optional IPv6
/// scope ID. See [`std::net::AddrParseError`].
pub enum AddrParseError {
    #[error(transparent)]
    IpInvalid(#[from] std::net::AddrParseError),
    #[error("no interface found with name \"{0}\"")]
    InterfaceNotFound(String),
    #[error("only IPv6 link-local may include a scope ID")]
    /// Scope IDs are only supported for IPv6 link-local addresses as per RFC
    /// 6874 Section 4.
    ScopeNotSupported,
}

impl std::str::FromStr for IpAddr {
    type Err = AddrParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.splitn(2, '%');
        let addr = parts.next().unwrap(); // first element is guaranteed
        let ip = std::net::IpAddr::from_str(addr)?;
        let scope = parts.next();
        match (ip, scope) {
            (std::net::IpAddr::V4(ip), None) => Ok(IpAddr::from(ip)),
            (std::net::IpAddr::V4(_), Some(_)) => Err(AddrParseError::ScopeNotSupported),
            (std::net::IpAddr::V6(ip), None) => Ok(IpAddr::V6(ip, None)),
            (std::net::IpAddr::V6(ip), Some(scope)) => {
                if !ip.is_link_local_addr() {
                    return Err(AddrParseError::ScopeNotSupported);
                }
                if scope.len() == 0 {
                    return Err(AddrParseError::InterfaceNotFound(scope.to_string()));
                }
                Ok(IpAddr::V6(ip, Some(scope.to_string())))
            }
        }
    }
}

impl Display for IpAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpAddr::V4(ip) => Display::fmt(ip, f),
            IpAddr::V6(ip, None) => Display::fmt(ip, f),
            IpAddr::V6(ip, Some(scope)) => {
                Display::fmt(ip, f)?;
                write!(f, "%{}", scope)
            }
        }
    }
}

impl Debug for IpAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, f)
    }
}

impl Serialize for IpAddr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.to_string().as_str())
    }
}

impl<'de> Deserialize<'de> for IpAddr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(FromStrVisitor::new())
    }
}

struct FromStrVisitor<T> {
    ty: PhantomData<T>,
}

impl<T> FromStrVisitor<T> {
    fn new() -> Self {
        FromStrVisitor { ty: PhantomData }
    }
}

impl<'de, T> serde::de::Visitor<'de> for FromStrVisitor<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    type Value = T;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("IP address")
    }

    fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod test {
    use super::{AddrParseError, IpAddr};
    use assert_matches::assert_matches;

    #[test]
    fn parse_ip_invalid() {
        assert_matches!("".parse::<IpAddr>(), Err(AddrParseError::IpInvalid(_)));
        assert_matches!("192.168.1.".parse::<IpAddr>(), Err(AddrParseError::IpInvalid(_)));
        assert_matches!("fe80:".parse::<IpAddr>(), Err(AddrParseError::IpInvalid(_)));
    }

    #[test]
    fn parse_ipv4() {
        assert_matches!(
            "192.168.1.1".parse::<IpAddr>(),
            Ok(IpAddr::V4(ip))
                if ip == "192.168.1.1".parse::<std::net::Ipv4Addr>().unwrap()
        );
    }

    #[test]
    fn parse_ipv4_with_scope() {
        assert_matches!("192.168.1.1%1".parse::<IpAddr>(), Err(AddrParseError::ScopeNotSupported));
    }

    #[test]
    fn parse_ipv6() {
        assert_matches!(
            "fe80::1".parse::<IpAddr>(),
            Ok(IpAddr::V6(ip, None))
                if ip == "fe80::1".parse::<std::net::Ipv6Addr>().unwrap()
        );
    }

    #[test]
    fn parse_ipv6_global_with_scope() {
        assert_matches!("2001::1%1".parse::<IpAddr>(), Err(AddrParseError::ScopeNotSupported));
    }

    #[test]
    fn parse_ipv6_link_local_with_scope() {
        assert_matches!(
            "fe80::1%1".parse::<IpAddr>(),
            Ok(IpAddr::V6(ip, Some(scope)))
                if ip == "fe80::1".parse::<std::net::Ipv6Addr>().unwrap()
                && scope == "1"
        );
    }

    #[test]
    fn parse_ipv6_link_local_with_scope_interface_not_found() {
        // An empty scope ID should trigger a failed lookup.
        assert_matches!(
            "fe80::1%".parse::<IpAddr>(),
            Err(AddrParseError::InterfaceNotFound(name))
                if name == ""
        );
    }
}
