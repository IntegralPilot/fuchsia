// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use pest::iterators::Pair;

use fidl_fuchsia_net as net;

use crate::grammar::{Error, Rule};

pub(crate) fn parse_invertible_subnet(pair: Pair<'_, Rule>) -> Result<(net::Subnet, bool), Error> {
    assert_eq!(pair.as_rule(), Rule::invertible_subnet);
    let mut inner = pair.into_inner();
    let mut pair = inner.next().unwrap();
    let invert_match = match pair.as_rule() {
        Rule::not => {
            pair = inner.next().unwrap();
            true
        }
        Rule::subnet => false,
        _ => unreachable!("invertible subnet must be either not or a subnet"),
    };
    let subnet = parse_subnet(pair)?;
    Ok((subnet, invert_match))
}

pub(crate) fn parse_subnet(pair: Pair<'_, Rule>) -> Result<net::Subnet, Error> {
    assert_eq!(pair.as_rule(), Rule::subnet);
    let mut inner = pair.into_inner();
    let addr = parse_ipaddr(inner.next().unwrap())?;
    let prefix_len = parse_prefix_len(inner.next().unwrap())?;

    Ok(net::Subnet { addr, prefix_len })
}

pub(crate) fn parse_ipaddr(pair: Pair<'_, Rule>) -> Result<net::IpAddress, Error> {
    assert_eq!(pair.as_rule(), Rule::ipaddr);
    let pair = pair.into_inner().next().unwrap();
    let addr = pair.as_str().parse().map_err(Error::Addr)?;
    match addr {
        std::net::IpAddr::V4(ip4) => {
            Ok(net::IpAddress::Ipv4(net::Ipv4Address { addr: ip4.octets() }))
        }
        std::net::IpAddr::V6(ip6) => {
            Ok(net::IpAddress::Ipv6(net::Ipv6Address { addr: ip6.octets() }))
        }
    }
}

pub(crate) fn parse_prefix_len(pair: Pair<'_, Rule>) -> Result<u8, Error> {
    assert_eq!(pair.as_rule(), Rule::prefix_len);
    pair.as_str().parse::<u8>().map_err(Error::Num)
}

pub(crate) fn parse_port_num(pair: Pair<'_, Rule>) -> Result<u16, Error> {
    pair.as_str().parse::<u16>().map_err(Error::Num)
}

pub(crate) fn ip_version_eq(left: &net::IpAddress, right: &net::IpAddress) -> bool {
    match (left, right) {
        (net::IpAddress::Ipv4(_), net::IpAddress::Ipv4(_))
        | (net::IpAddress::Ipv6(_), net::IpAddress::Ipv6(_)) => true,
        (net::IpAddress::Ipv4(_), net::IpAddress::Ipv6(_))
        | (net::IpAddress::Ipv6(_), net::IpAddress::Ipv4(_)) => false,
    }
}
