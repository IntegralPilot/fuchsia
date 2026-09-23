// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use pest::Parser;
use pest::iterators::Pair;

use fidl_fuchsia_hardware_network as fhnet;
use fidl_fuchsia_net as net;
use fidl_fuchsia_net_filter_deprecated as filter;

use crate::grammar::{Error, FilterRuleParser, InvalidReason, Rule};
use crate::util;

fn parse_action(pair: Pair<'_, Rule>) -> filter::Action {
    assert_eq!(pair.as_rule(), Rule::action);
    match pair.into_inner().next().unwrap().as_rule() {
        Rule::pass => filter::Action::Pass,
        Rule::drop => filter::Action::Drop,
        Rule::dropreset => filter::Action::DropReset,
        _ => unreachable!("action must be one of (pass|drop|dropreset)"),
    }
}

fn parse_direction(pair: Pair<'_, Rule>) -> filter::Direction {
    assert_eq!(pair.as_rule(), Rule::direction);
    match pair.into_inner().next().unwrap().as_rule() {
        Rule::incoming => filter::Direction::Incoming,
        Rule::outgoing => filter::Direction::Outgoing,
        _ => unreachable!("direction must be one of (in|out)"),
    }
}

fn parse_proto(pair: Pair<'_, Rule>) -> filter::SocketProtocol {
    assert_eq!(pair.as_rule(), Rule::proto);
    match pair.into_inner().next() {
        Some(pair) => match pair.as_rule() {
            Rule::tcp => filter::SocketProtocol::Tcp,
            Rule::udp => filter::SocketProtocol::Udp,
            Rule::icmp => filter::SocketProtocol::Icmp,
            _ => unreachable!("protocol must be one of (tcp|udp|icmp)"),
        },
        None => filter::SocketProtocol::Any,
    }
}

fn parse_devclass(pair: Pair<'_, Rule>) -> filter::DeviceClass {
    assert_eq!(pair.as_rule(), Rule::devclass);
    match pair.into_inner().next() {
        Some(pair) => match pair.as_rule() {
            Rule::virt => filter::DeviceClass::Match_(fhnet::PortClass::Virtual),
            Rule::ethernet => filter::DeviceClass::Match_(fhnet::PortClass::Ethernet),
            Rule::wlan => filter::DeviceClass::Match_(fhnet::PortClass::WlanClient),
            Rule::ppp => filter::DeviceClass::Match_(fhnet::PortClass::Ppp),
            Rule::bridge => filter::DeviceClass::Match_(fhnet::PortClass::Bridge),
            Rule::ap => filter::DeviceClass::Match_(fhnet::PortClass::WlanAp),
            Rule::lowpan => filter::DeviceClass::Match_(fhnet::PortClass::Lowpan),
            _ => unreachable!("devclass must be one of (virt|ethernet|wlan|ppp|bridge|ap|lowpan)"),
        },
        None => filter::DeviceClass::Any(filter::Empty {}),
    }
}

fn parse_src(
    pair: Pair<'_, Rule>,
) -> Result<(Option<Box<net::Subnet>>, bool, filter::PortRange), Error> {
    assert_eq!(pair.as_rule(), Rule::src);
    parse_src_or_dst(pair)
}

fn parse_dst(
    pair: Pair<'_, Rule>,
) -> Result<(Option<Box<net::Subnet>>, bool, filter::PortRange), Error> {
    assert_eq!(pair.as_rule(), Rule::dst);
    parse_src_or_dst(pair)
}

fn parse_src_or_dst(
    pair: Pair<'_, Rule>,
) -> Result<(Option<Box<net::Subnet>>, bool, filter::PortRange), Error> {
    let mut inner = pair.into_inner();
    match inner.next() {
        Some(pair) => match pair.as_rule() {
            Rule::invertible_subnet => {
                let (subnet, invert_match) = util::parse_invertible_subnet(pair)?;
                let port = match inner.next() {
                    Some(pair) => parse_port_range(pair)?,
                    None => filter::PortRange { start: 0, end: 0 },
                };
                Ok((Some(Box::new(subnet)), invert_match, port))
            }
            Rule::port_range => Ok((None, false, parse_port_range(pair)?)),
            _ => unreachable!("src or dst must be either an invertible subnet or port range"),
        },
        None => Ok((None, false, filter::PortRange { start: 0, end: 0 })),
    }
}

fn parse_port_range(pair: Pair<'_, Rule>) -> Result<filter::PortRange, Error> {
    assert_eq!(pair.as_rule(), Rule::port_range);
    let mut inner = pair.into_inner();
    let pair = inner.next().unwrap();
    match pair.as_rule() {
        Rule::port => {
            let port_num = util::parse_port_num(inner.next().unwrap())?;
            Ok(filter::PortRange { start: port_num, end: port_num })
        }
        Rule::range => {
            let port_start = util::parse_port_num(inner.next().unwrap())?;
            let port_end = util::parse_port_num(inner.next().unwrap())?;
            Ok(filter::PortRange { start: port_start, end: port_end })
        }
        _ => unreachable!("port range must be either a single port, or a port range"),
    }
}

fn parse_log(pair: Pair<'_, Rule>) -> bool {
    assert_eq!(pair.as_rule(), Rule::log);
    pair.as_str() == "log"
}

fn parse_state(pair: Pair<'_, Rule>) -> bool {
    assert_eq!(pair.as_rule(), Rule::state);
    let mut inner = pair.into_inner();
    match inner.next() {
        Some(pair) => {
            assert_eq!(pair.as_rule(), Rule::state_adj);
            match pair.as_str() {
                "no" => false,
                "keep" => true,
                _ => unreachable!("state must be either (no|keep)"),
            }
        }
        None => false, // no state by default
    }
}

fn parse_rule(pair: Pair<'_, Rule>) -> Result<filter::Rule, Error> {
    assert_eq!(pair.as_rule(), Rule::rule);
    let mut pairs = pair.into_inner();

    let action = parse_action(pairs.next().unwrap());
    let direction = parse_direction(pairs.next().unwrap());
    let proto = parse_proto(pairs.next().unwrap());
    let device_class = parse_devclass(pairs.next().unwrap());
    let (src_subnet, src_subnet_invert_match, src_port_range) = parse_src(pairs.next().unwrap())?;
    let (dst_subnet, dst_subnet_invert_match, dst_port_range) = parse_dst(pairs.next().unwrap())?;
    let log = parse_log(pairs.next().unwrap());
    let keep_state = parse_state(pairs.next().unwrap());

    Ok(filter::Rule {
        action,
        direction,
        proto,
        src_subnet,
        src_subnet_invert_match,
        src_port_range,
        dst_subnet,
        dst_subnet_invert_match,
        dst_port_range,
        nic: 0, // TODO: Support NICID (currently always 0 (= any))
        log,
        keep_state,
        device_class,
    })
}

fn parse_nat(pair: Pair<'_, Rule>) -> Result<filter::Nat, Error> {
    assert_eq!(pair.as_rule(), Rule::nat);
    let mut pairs = pair.into_inner();

    let proto = parse_proto(pairs.next().unwrap());
    let src_subnet = util::parse_subnet(pairs.next().unwrap())?;

    Ok(filter::Nat {
        proto,
        src_subnet,
        outgoing_nic: 0, // TODO: Support NICID.
    })
}

fn parse_rdr(pair: Pair<'_, Rule>) -> Result<filter::Rdr, Error> {
    assert_eq!(pair.as_rule(), Rule::rdr);
    let mut pairs = pair.into_inner();

    let proto = parse_proto(pairs.next().unwrap());
    let dst_addr = util::parse_ipaddr(pairs.next().unwrap())?;
    let dst_port_range = parse_port_range(pairs.next().unwrap())?;
    let new_dst_addr = util::parse_ipaddr(pairs.next().unwrap())?;
    let new_dst_port_range = parse_port_range(pairs.next().unwrap())?;

    Ok(filter::Rdr {
        proto,
        dst_addr,
        dst_port_range,
        new_dst_addr,
        new_dst_port_range,
        nic: 0, // TODO: Support NICID.
    })
}

fn validate_port_range(range: &filter::PortRange) -> Result<(), Error> {
    if range.start > range.end {
        return Err(Error::Invalid(InvalidReason::InvalidPortRange));
    }
    Ok(())
}

fn port_range_length(range: &filter::PortRange) -> Result<u16, Error> {
    validate_port_range(&range)?;
    Ok(range.end - range.start)
}

fn validate_rule(rule: &filter::Rule) -> Result<(), Error> {
    if let (Some(src_subnet), Some(dst_subnet)) = (&rule.src_subnet, &rule.dst_subnet) {
        if !util::ip_version_eq(&src_subnet.addr, &dst_subnet.addr) {
            return Err(Error::Invalid(InvalidReason::MixedIPVersions));
        }
    }
    validate_port_range(&rule.src_port_range)?;
    validate_port_range(&rule.dst_port_range)?;

    Ok(())
}

fn validate_rdr(rdr: &filter::Rdr) -> Result<(), Error> {
    if !util::ip_version_eq(&rdr.dst_addr, &rdr.new_dst_addr) {
        return Err(Error::Invalid(InvalidReason::MixedIPVersions));
    }
    if port_range_length(&rdr.dst_port_range)? != port_range_length(&rdr.new_dst_port_range)? {
        return Err(Error::Invalid(InvalidReason::PortRangeLengthMismatch));
    }

    Ok(())
}

pub fn parse_str_to_rules(line: &str) -> Result<Vec<filter::Rule>, Error> {
    let mut pairs =
        FilterRuleParser::parse(Rule::rules, &line).map_err(|err| Error::Pest(Box::new(err)))?;
    let mut rules = Vec::new();
    for filter_rule in pairs.next().unwrap().into_inner() {
        match filter_rule.as_rule() {
            Rule::rule => {
                let rule = parse_rule(filter_rule)?;
                validate_rule(&rule)?;
                rules.push(rule);
            }
            Rule::EOI => (),
            _ => unreachable!("rule must only have a rule case"),
        }
    }
    Ok(rules)
}

pub fn parse_str_to_nat_rules(line: &str) -> Result<Vec<filter::Nat>, Error> {
    let mut pairs = FilterRuleParser::parse(Rule::nat_rules, &line)
        .map_err(|err| Error::Pest(Box::new(err)))?;
    let mut nat_rules = Vec::new();
    for filter_rule in pairs.next().unwrap().into_inner() {
        match filter_rule.as_rule() {
            Rule::nat => {
                let nat = parse_nat(filter_rule)?;
                nat_rules.push(nat);
            }
            Rule::EOI => (),
            _ => unreachable!("nat must only have a nat case"),
        }
    }
    Ok(nat_rules)
}

pub fn parse_str_to_rdr_rules(line: &str) -> Result<Vec<filter::Rdr>, Error> {
    let mut pairs = FilterRuleParser::parse(Rule::rdr_rules, &line)
        .map_err(|err| Error::Pest(Box::new(err)))?;
    let mut rdr_rules = Vec::new();
    for filter_rule in pairs.next().unwrap().into_inner() {
        match filter_rule.as_rule() {
            Rule::rdr => {
                let rdr = parse_rdr(filter_rule)?;
                validate_rdr(&rdr)?;
                rdr_rules.push(rdr);
            }
            Rule::EOI => (),
            _ => unreachable!("rdr must only have a rdr case"),
        }
    }
    Ok(rdr_rules)
}

#[cfg(test)]
mod test {
    use super::*;

    use net_declare::{fidl_ip, fidl_subnet};

    #[test]
    fn test_rule_with_proto_any() {
        assert_eq!(
            parse_str_to_rules("pass in;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Any,
                src_subnet: None,
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 0, end: 0 },
                dst_subnet: None,
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 0, end: 0 },
                nic: 0,
                log: false,
                keep_state: false,
                device_class: filter::DeviceClass::Any(filter::Empty {}),
            }])
        );
    }

    #[test]
    fn test_rule_with_proto_tcp() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: None,
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 0, end: 0 },
                dst_subnet: None,
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 0, end: 0 },
                nic: 0,
                log: false,
                keep_state: false,
                device_class: filter::DeviceClass::Any(filter::Empty {}),
            }])
        );
    }

    #[test]
    fn test_multiple_rules() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp; drop out proto udp;"),
            Ok(vec![
                filter::Rule {
                    action: filter::Action::Pass,
                    direction: filter::Direction::Incoming,
                    proto: filter::SocketProtocol::Tcp,
                    src_subnet: None,
                    src_subnet_invert_match: false,
                    src_port_range: filter::PortRange { start: 0, end: 0 },
                    dst_subnet: None,
                    dst_subnet_invert_match: false,
                    dst_port_range: filter::PortRange { start: 0, end: 0 },
                    nic: 0,
                    log: false,
                    keep_state: false,
                    device_class: filter::DeviceClass::Any(filter::Empty {}),
                },
                filter::Rule {
                    action: filter::Action::Drop,
                    direction: filter::Direction::Outgoing,
                    proto: filter::SocketProtocol::Udp,
                    src_subnet: None,
                    src_subnet_invert_match: false,
                    src_port_range: filter::PortRange { start: 0, end: 0 },
                    dst_subnet: None,
                    dst_subnet_invert_match: false,
                    dst_port_range: filter::PortRange { start: 0, end: 0 },
                    nic: 0,
                    log: false,
                    keep_state: false,
                    device_class: filter::DeviceClass::Any(filter::Empty {}),
                },
            ])
        );
    }

    #[test]
    fn test_rule_with_from_v4_address() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp from 1.2.3.4/24;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: Some(Box::new(fidl_subnet!("1.2.3.4/24"))),
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 0, end: 0 },
                dst_subnet: None,
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 0, end: 0 },
                nic: 0,
                log: false,
                keep_state: false,
                device_class: filter::DeviceClass::Any(filter::Empty {}),
            }])
        );
    }

    #[test]
    fn test_rule_with_from_port() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp from port 10000;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: None,
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 10000, end: 10000 },
                dst_subnet: None,
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 0, end: 0 },
                nic: 0,
                log: false,
                keep_state: false,
                device_class: filter::DeviceClass::Any(filter::Empty {}),
            }])
        );
    }

    #[test]
    fn test_rule_with_from_range() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp from range 10000:10010;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: None,
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 10000, end: 10010 },
                dst_subnet: None,
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 0, end: 0 },
                nic: 0,
                log: false,
                keep_state: false,
                device_class: filter::DeviceClass::Any(filter::Empty {}),
            }])
        );
    }

    #[test]
    fn test_rule_with_from_range_2() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp from range 0:5;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: None,
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 0, end: 5 },
                dst_subnet: None,
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 0, end: 0 },
                nic: 0,
                log: false,
                keep_state: false,
                device_class: filter::DeviceClass::Any(filter::Empty {}),
            }])
        );
    }

    #[test]
    fn test_rule_with_from_invalid_range() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp from range 10005:10000;"),
            Err(Error::Invalid(InvalidReason::InvalidPortRange))
        );
    }

    #[test]
    fn test_rule_with_from_v4_address_port() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp from 1.2.3.4/24 port 10000;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: Some(Box::new(fidl_subnet!("1.2.3.4/24"))),
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 10000, end: 10000 },
                dst_subnet: None,
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 0, end: 0 },
                nic: 0,
                log: false,
                keep_state: false,
                device_class: filter::DeviceClass::Any(filter::Empty {}),
            }])
        );
    }

    #[test]
    fn test_rule_with_from_not_v4_address_port() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp from !1.2.3.4/24 port 10000;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: Some(Box::new(fidl_subnet!("1.2.3.4/24"))),
                src_subnet_invert_match: true,
                src_port_range: filter::PortRange { start: 10000, end: 10000 },
                dst_subnet: None,
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 0, end: 0 },
                nic: 0,
                log: false,
                keep_state: false,
                device_class: filter::DeviceClass::Any(filter::Empty {}),
            }])
        );
    }

    #[test]
    fn test_rule_with_from_v6_address_port() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp from 1234:5678::/32 port 10000;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: Some(Box::new(fidl_subnet!("1234:5678::/32"))),
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 10000, end: 10000 },
                dst_subnet: None,
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 0, end: 0 },
                nic: 0,
                log: false,
                keep_state: false,
                device_class: filter::DeviceClass::Any(filter::Empty {}),
            }])
        );
    }

    #[test]
    fn test_rule_with_to_v6_address_port() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp to 1234:5678::/32 port 10000;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: None,
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 0, end: 0 },
                dst_subnet: Some(Box::new(fidl_subnet!("1234:5678::/32"))),
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 10000, end: 10000 },
                nic: 0,
                log: false,
                keep_state: false,
                device_class: filter::DeviceClass::Any(filter::Empty {}),
            }])
        );
    }

    #[test]
    fn test_rule_with_from_v6_address_port_to_v4_address_port() {
        assert_eq!(
            parse_str_to_rules(
                "pass in proto tcp from 1234:5678::/32 port 10000 to 1.2.3.4/8 port 1000;"
            ),
            Err(Error::Invalid(InvalidReason::MixedIPVersions))
        );
    }

    #[test]
    fn test_rule_with_from_v6_address_port_to_v6_address_port() {
        assert_eq!(
            parse_str_to_rules(
                "pass in proto tcp from 1234:5678::/32 port 10000 to 2345:6789::/32 port 1000;"
            ),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: Some(Box::new(fidl_subnet!("1234:5678::/32"))),
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 10000, end: 10000 },
                dst_subnet: Some(Box::new(fidl_subnet!("2345:6789::/32"))),
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 1000, end: 1000 },
                nic: 0,
                log: false,
                keep_state: false,
                device_class: filter::DeviceClass::Any(filter::Empty {}),
            }])
        );
    }

    #[test]
    fn test_rule_with_log_no_state() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp log no state;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: None,
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 0, end: 0 },
                dst_subnet: None,
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 0, end: 0 },
                nic: 0,
                log: true,
                keep_state: false,
                device_class: filter::DeviceClass::Any(filter::Empty {}),
            }])
        );
    }

    #[test]
    fn test_rule_with_keep_state() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp keep state;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: None,
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 0, end: 0 },
                dst_subnet: None,
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 0, end: 0 },
                nic: 0,
                log: false,
                keep_state: true,
                device_class: filter::DeviceClass::Any(filter::Empty {}),
            }])
        );
    }

    #[test]
    fn test_rule_with_device_class() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp devclass ap;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: None,
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 0, end: 0 },
                dst_subnet: None,
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 0, end: 0 },
                nic: 0,
                log: false,
                keep_state: false,
                device_class: filter::DeviceClass::Match_(fhnet::PortClass::WlanAp),
            }])
        );
    }

    #[test]
    fn test_rule_with_device_class_and_dst_range() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp devclass ap to range 1:2;"),
            Ok(vec![filter::Rule {
                action: filter::Action::Pass,
                direction: filter::Direction::Incoming,
                proto: filter::SocketProtocol::Tcp,
                src_subnet: None,
                src_subnet_invert_match: false,
                src_port_range: filter::PortRange { start: 0, end: 0 },
                dst_subnet: None,
                dst_subnet_invert_match: false,
                dst_port_range: filter::PortRange { start: 1, end: 2 },
                nic: 0,
                log: false,
                keep_state: false,
                device_class: filter::DeviceClass::Match_(fhnet::PortClass::WlanAp),
            }])
        );
    }

    #[test]
    fn test_nat_rule_from_v4_subnet() {
        assert_eq!(
            parse_str_to_nat_rules("nat from 1.2.3.0/24 -> from 192.168.1.1;"),
            Ok(vec![filter::Nat {
                proto: filter::SocketProtocol::Any,
                src_subnet: fidl_subnet!("1.2.3.0/24"),
                outgoing_nic: 0,
            }])
        );
    }

    #[test]
    fn test_nat_rule_with_proto_tcp_from_v4_subnet() {
        assert_eq!(
            parse_str_to_nat_rules("nat proto tcp from 1.2.3.0/24 -> from 192.168.1.1;"),
            Ok(vec![filter::Nat {
                proto: filter::SocketProtocol::Tcp,
                src_subnet: fidl_subnet!("1.2.3.0/24"),
                outgoing_nic: 0,
            }])
        );
    }

    #[test]
    fn test_rdr_rule_with_to_v4_address_port_to_v4_address_port() {
        assert_eq!(
            parse_str_to_rdr_rules("rdr to 1.2.3.4 port 10000 -> to 192.168.1.1 port 20000;"),
            Ok(vec![filter::Rdr {
                proto: filter::SocketProtocol::Any,
                dst_addr: fidl_ip!("1.2.3.4"),
                dst_port_range: filter::PortRange { start: 10000, end: 10000 },
                new_dst_addr: fidl_ip!("192.168.1.1"),
                new_dst_port_range: filter::PortRange { start: 20000, end: 20000 },
                nic: 0,
            }])
        );
    }

    #[test]
    fn test_rdr_rule_with_proto_tcp_to_v4_address_port_to_v4_address_port() {
        assert_eq!(
            parse_str_to_rdr_rules(
                "rdr proto tcp to 1.2.3.4 port 10000 -> to 192.168.1.1 port 20000;"
            ),
            Ok(vec![filter::Rdr {
                proto: filter::SocketProtocol::Tcp,
                dst_addr: fidl_ip!("1.2.3.4"),
                dst_port_range: filter::PortRange { start: 10000, end: 10000 },
                new_dst_addr: fidl_ip!("192.168.1.1"),
                new_dst_port_range: filter::PortRange { start: 20000, end: 20000 },
                nic: 0,
            }])
        );
    }

    #[test]
    fn test_rdr_rule_with_to_v4_address_range_to_v4_address_range() {
        assert_eq!(
            parse_str_to_rdr_rules(
                "rdr proto tcp to 1.2.3.4 range 10000:10005 -> to 192.168.1.1 range 20000:20005;"
            ),
            Ok(vec![filter::Rdr {
                proto: filter::SocketProtocol::Tcp,
                dst_addr: fidl_ip!("1.2.3.4"),
                dst_port_range: filter::PortRange { start: 10000, end: 10005 },
                new_dst_addr: fidl_ip!("192.168.1.1"),
                new_dst_port_range: filter::PortRange { start: 20000, end: 20005 },
                nic: 0,
            }])
        );
    }

    #[test]
    fn test_rdr_rule_with_to_v4_address_range_to_v4_address_invalid_range() {
        assert_eq!(
            parse_str_to_rdr_rules(
                "rdr proto tcp to 1.2.3.4 range 10000:10005 -> to 192.168.1.1 range 20005:20000;"
            ),
            Err(Error::Invalid(InvalidReason::InvalidPortRange))
        );
    }

    #[test]
    fn test_rdr_rule_with_to_v4_address_range_to_v4_address_port_range_length_mismatch() {
        assert_eq!(
            parse_str_to_rdr_rules(
                "rdr proto tcp to 1.2.3.4 range 10000:10005 -> to 192.168.1.1 range 20000:20003;"
            ),
            Err(Error::Invalid(InvalidReason::PortRangeLengthMismatch))
        );
    }

    #[test]
    fn test_rdr_rule_with_to_v4_address_range_to_v6_address_range() {
        assert_eq!(
            parse_str_to_rdr_rules(
                "rdr proto tcp to 1.2.3.4 range 10000:10005 -> to 1234:5678:: range 20000:20005;"
            ),
            Err(Error::Invalid(InvalidReason::MixedIPVersions))
        );
    }

    #[test]
    fn test_rdr_rule_with_to_v6_address_range_to_v6_address_range() {
        assert_eq!(
            parse_str_to_rdr_rules(
                "rdr proto tcp to 1234:5678:: range 10000:10005 -> to 2345:6789:: range 20000:20005;"
            ),
            Ok(vec![filter::Rdr {
                proto: filter::SocketProtocol::Tcp,
                dst_addr: fidl_ip!("1234:5678::"),
                dst_port_range: filter::PortRange { start: 10000, end: 10005 },
                new_dst_addr: fidl_ip!("2345:6789::"),
                new_dst_port_range: filter::PortRange { start: 20000, end: 20005 },
                nic: 0,
            }])
        );
    }
}
