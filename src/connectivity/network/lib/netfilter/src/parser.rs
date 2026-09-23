// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use pest::Parser;
use pest::iterators::Pair;

use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_filter_ext as filter_ext;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;

use crate::grammar::{Error, FilterRuleParser, InvalidReason, Rule};
use crate::util;

fn parse_action(pair: Pair<'_, Rule>) -> filter_ext::Action {
    assert_eq!(pair.as_rule(), Rule::action);
    match pair.into_inner().next().unwrap().as_rule() {
        Rule::pass => filter_ext::Action::Accept,
        Rule::drop => filter_ext::Action::Drop,
        // TODO(https://fxbug.dev/329500057): Remove dropreset from the grammar
        // as it is unimplemented in filter.deprecated and currently unsupported
        // in the filter2 API
        Rule::dropreset => todo!("not yet supported in the filter2 API"),
        _ => unreachable!("action must be one of (pass|drop|dropreset)"),
    }
}

// A subset of `filter_ext::IpHook` to handle current
// parsing capabilities.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Direction {
    LocalIngress,
    LocalEgress,
}

fn parse_direction(pair: Pair<'_, Rule>) -> Direction {
    assert_eq!(pair.as_rule(), Rule::direction);
    match pair.into_inner().next().unwrap().as_rule() {
        Rule::incoming => Direction::LocalIngress,
        Rule::outgoing => Direction::LocalEgress,
        _ => unreachable!("direction must be one of (in|out)"),
    }
}

// A subset of `fnet_matchers_ext::TransportProtocol` to handle current
// parsing capabilities.
enum TransportProtocol {
    Tcp,
    Udp,
    Icmp,
}

fn parse_proto(pair: Pair<'_, Rule>) -> Option<TransportProtocol> {
    assert_eq!(pair.as_rule(), Rule::proto);
    pair.into_inner().next().map(|pair| match pair.as_rule() {
        Rule::tcp => TransportProtocol::Tcp,
        Rule::udp => TransportProtocol::Udp,
        Rule::icmp => TransportProtocol::Icmp,
        _ => unreachable!("protocol must be one of (tcp|udp|icmp)"),
    })
}

fn parse_devclass(pair: Pair<'_, Rule>) -> Option<fnet_interfaces_ext::PortClass> {
    assert_eq!(pair.as_rule(), Rule::devclass);
    pair.into_inner().next().map(|pair| match pair.as_rule() {
        Rule::virt => fnet_interfaces_ext::PortClass::Virtual,
        Rule::ethernet => fnet_interfaces_ext::PortClass::Ethernet,
        Rule::wlan => fnet_interfaces_ext::PortClass::WlanClient,
        Rule::ppp => fnet_interfaces_ext::PortClass::Ppp,
        Rule::bridge => fnet_interfaces_ext::PortClass::Bridge,
        Rule::ap => fnet_interfaces_ext::PortClass::WlanAp,
        Rule::lowpan => fnet_interfaces_ext::PortClass::Lowpan,
        _ => unreachable!("devclass must be one of (virt|ethernet|wlan|ppp|bridge|ap|lowpan)"),
    })
}

fn parse_src(
    pair: Pair<'_, Rule>,
) -> Result<(Option<fnet_matchers_ext::Address>, Option<fnet_matchers_ext::Port>), Error> {
    assert_eq!(pair.as_rule(), Rule::src);
    parse_src_or_dst(pair)
}

fn parse_dst(
    pair: Pair<'_, Rule>,
) -> Result<(Option<fnet_matchers_ext::Address>, Option<fnet_matchers_ext::Port>), Error> {
    assert_eq!(pair.as_rule(), Rule::dst);
    parse_src_or_dst(pair)
}

fn parse_src_or_dst(
    pair: Pair<'_, Rule>,
) -> Result<(Option<fnet_matchers_ext::Address>, Option<fnet_matchers_ext::Port>), Error> {
    let mut inner = pair.into_inner();
    match inner.next() {
        Some(pair) => match pair.as_rule() {
            Rule::invertible_subnet => {
                let (subnet, invert_match) = util::parse_invertible_subnet(pair)?;
                let port = match inner.next() {
                    Some(pair) => Some(parse_port_range(pair)?),
                    None => None,
                };
                Ok((
                    Some(fnet_matchers_ext::Address {
                        matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                            fnet_matchers_ext::Subnet::try_from(subnet)
                                .map_err(|err| Error::Fidl(err.into()))?,
                        ),
                        invert: invert_match,
                    }),
                    port,
                ))
            }
            Rule::port_range => Ok((None, Some(parse_port_range(pair)?))),
            _ => unreachable!("src or dst must be either an invertible subnet or port range"),
        },
        None => Ok((None, None)),
    }
}

fn parse_port_range(pair: Pair<'_, Rule>) -> Result<fnet_matchers_ext::Port, Error> {
    assert_eq!(pair.as_rule(), Rule::port_range);
    let mut inner = pair.into_inner();
    let pair = inner.next().unwrap();
    match pair.as_rule() {
        Rule::port => {
            let port_num = util::parse_port_num(inner.next().unwrap())?;
            fnet_matchers_ext::Port::new(port_num, port_num, false).map_err(|err| match err {
                fnet_matchers_ext::PortError::InvalidPortRange => {
                    Error::Invalid(InvalidReason::InvalidPortRange)
                }
            })
        }
        Rule::range => {
            let port_start = util::parse_port_num(inner.next().unwrap())?;
            let port_end = util::parse_port_num(inner.next().unwrap())?;
            fnet_matchers_ext::Port::new(port_start, port_end, false).map_err(|err| match err {
                fnet_matchers_ext::PortError::InvalidPortRange => {
                    Error::Invalid(InvalidReason::InvalidPortRange)
                }
            })
        }
        _ => unreachable!("port range must be either a single port, or a port range"),
    }
}

fn parse_rule(
    pair: Pair<'_, Rule>,
    routines: &FilterRoutines,
    index: usize,
) -> Result<filter_ext::Rule, Error> {
    assert_eq!(pair.as_rule(), Rule::rule);
    let mut pairs = pair.into_inner();

    let action = parse_action(pairs.next().unwrap());
    let direction = parse_direction(pairs.next().unwrap());
    let proto = parse_proto(pairs.next().unwrap());
    let port_class = parse_devclass(pairs.next().unwrap());
    let mut in_interface = None;
    let mut out_interface = None;
    let routine_id = match direction {
        Direction::LocalIngress => {
            // Use the same RoutineId as the LocalIngress routine.
            let Some(ref routine_id) = routines.local_ingress else {
                return Err(Error::RoutineNotProvided(direction));
            };
            in_interface = port_class.map(|class| fnet_matchers_ext::Interface::PortClass(class));
            routine_id
        }
        Direction::LocalEgress => {
            // Use the same RoutineId as the LocalEgress routine.
            let Some(ref routine_id) = routines.local_egress else {
                return Err(Error::RoutineNotProvided(direction));
            };
            out_interface = port_class.map(|class| fnet_matchers_ext::Interface::PortClass(class));
            routine_id
        }
    };
    let (src_addr, src_port) = parse_src(pairs.next().unwrap())?;
    let (dst_addr, dst_port) = parse_dst(pairs.next().unwrap())?;
    let transport_protocol = proto.map(|proto| match proto {
        TransportProtocol::Tcp => fnet_matchers_ext::TransportProtocol::Tcp { src_port, dst_port },
        TransportProtocol::Udp => fnet_matchers_ext::TransportProtocol::Udp { src_port, dst_port },
        TransportProtocol::Icmp => fnet_matchers_ext::TransportProtocol::Icmp,
    });

    Ok(filter_ext::Rule {
        id: filter_ext::RuleId { routine: routine_id.clone(), index: index as u32 },
        matchers: filter_ext::Matchers {
            in_interface,
            out_interface,
            src_addr,
            dst_addr,
            transport_protocol,
            ..Default::default()
        },
        action,
    })
}

// A container for `filter_ext::Routine`s that back the
// `filter_ext::IpInstallationHook`s currently supported
// by the parser.
#[derive(Debug, Default)]
pub struct FilterRoutines {
    pub local_ingress: Option<filter_ext::RoutineId>,
    pub local_egress: Option<filter_ext::RoutineId>,
}

// A container for `filter_ext::Routine`s that back the
// `filter_ext::NatInstallationHook`s currently supported
// by the parser.
#[derive(Debug, Default)]
pub struct NatRoutines {}

fn validate_rule(rule: &filter_ext::Rule) -> Result<(), Error> {
    if let (Some(src_subnet), Some(dst_subnet)) = (&rule.matchers.src_addr, &rule.matchers.dst_addr)
    {
        if let (
            fnet_matchers_ext::AddressMatcherType::Subnet(src_subnet),
            fnet_matchers_ext::AddressMatcherType::Subnet(dst_subnet),
        ) = (&src_subnet.matcher, &dst_subnet.matcher)
        {
            if !util::ip_version_eq(
                &fnet::Subnet::from(*src_subnet).addr,
                &fnet::Subnet::from(*dst_subnet).addr,
            ) {
                return Err(Error::Invalid(InvalidReason::MixedIPVersions));
            }
        }
    }

    Ok(())
}

pub fn parse_str_to_rules(
    line: &str,
    routines: &FilterRoutines,
) -> Result<Vec<filter_ext::Rule>, Error> {
    let mut pairs =
        FilterRuleParser::parse(Rule::rules, &line).map_err(|err| Error::Pest(Box::new(err)))?;
    let mut rules = Vec::new();
    for (index, filter_rule) in pairs.next().unwrap().into_inner().into_iter().enumerate() {
        match filter_rule.as_rule() {
            Rule::rule => {
                let rule = parse_rule(filter_rule, &routines, index)?;
                validate_rule(&rule)?;
                rules.push(rule);
            }
            Rule::EOI => (),
            _ => unreachable!("rule must only have a rule case"),
        }
    }
    Ok(rules)
}

pub fn parse_str_to_nat_rules(
    _line: &str,
    _routines: &NatRoutines,
) -> Result<Vec<filter_ext::Rule>, Error> {
    // TODO(https://fxbug.dev/323950204): Parse NAT rules once
    // supported in filter2
    todo!("not yet supported in the filter2 API")
}

pub fn parse_str_to_rdr_rules(
    _line: &str,
    _routines: &NatRoutines,
) -> Result<Vec<filter_ext::Rule>, Error> {
    // TODO(https://fxbug.dev/323949893): Parse NAT RDR rules once
    // supported in filter2
    todo!("not yet supported in the filter2 API")
}

#[cfg(test)]
mod test {
    use super::*;

    use net_declare::fidl_subnet;

    fn test_filter_routines() -> FilterRoutines {
        FilterRoutines {
            local_ingress: Some(local_ingress_routine()),
            local_egress: Some(local_egress_routine()),
        }
    }

    fn local_ingress_routine() -> filter_ext::RoutineId {
        test_routine_id("local_ingress")
    }

    fn local_egress_routine() -> filter_ext::RoutineId {
        test_routine_id("local_egress")
    }

    fn test_routine_id(name: &str) -> filter_ext::RoutineId {
        filter_ext::RoutineId {
            namespace: filter_ext::NamespaceId(String::from("namespace")),
            name: String::from(name),
        }
    }

    #[test]
    fn test_rule_with_proto_any() {
        assert_eq!(
            parse_str_to_rules("pass in;", &test_filter_routines()),
            Ok(vec![filter_ext::Rule {
                id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                matchers: filter_ext::Matchers::default(),
                action: filter_ext::Action::Accept,
            }])
        );
    }

    #[test]
    fn test_rule_local_ingress_without_corresponding_routine() {
        assert_eq!(
            parse_str_to_rules("pass in;", &FilterRoutines::default()),
            Err(Error::RoutineNotProvided(Direction::LocalIngress))
        );
    }

    #[test]
    fn test_rule_local_egress_without_corresponding_routine() {
        assert_eq!(
            parse_str_to_rules("pass out;", &FilterRoutines::default()),
            Err(Error::RoutineNotProvided(Direction::LocalEgress))
        );
    }

    #[test]
    fn test_rule_with_proto_tcp() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp;", &test_filter_routines()),
            Ok(vec![filter_ext::Rule {
                id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                matchers: filter_ext::Matchers {
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                        src_port: None,
                        dst_port: None,
                    }),
                    ..Default::default()
                },
                action: filter_ext::Action::Accept,
            }])
        )
    }

    #[test]
    fn test_multiple_rules() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp; drop out proto udp;", &test_filter_routines()),
            Ok(vec![
                filter_ext::Rule {
                    id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                    matchers: filter_ext::Matchers {
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                            src_port: None,
                            dst_port: None,
                        }),
                        ..Default::default()
                    },
                    action: filter_ext::Action::Accept,
                },
                filter_ext::Rule {
                    id: filter_ext::RuleId { routine: local_egress_routine(), index: 1 },
                    matchers: filter_ext::Matchers {
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Udp {
                            src_port: None,
                            dst_port: None,
                        }),
                        ..Default::default()
                    },
                    action: filter_ext::Action::Drop,
                }
            ])
        )
    }

    #[test]
    fn test_rule_with_from_v4_address() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp from 1.2.3.0/24;", &test_filter_routines()),
            Ok(vec![
                filter_ext::Rule {
                    id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                    matchers: filter_ext::Matchers {
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                            src_port: None,
                            dst_port: None,
                        }),
                        src_addr: Some(fnet_matchers_ext::Address {
                            matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                                fnet_matchers_ext::Subnet::try_from(fidl_subnet!("1.2.3.0/24"))
                                    .unwrap()
                            ),
                            invert: false,
                        }),
                        ..Default::default()
                    },
                    action: filter_ext::Action::Accept,
                }
                .into()
            ])
        )
    }

    #[test]
    fn test_rule_with_from_port() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp from port 10000;", &test_filter_routines()),
            Ok(vec![
                filter_ext::Rule {
                    id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                    matchers: filter_ext::Matchers {
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                            src_port: Some(
                                fnet_matchers_ext::Port::new(10000, 10000, false).unwrap()
                            ),
                            dst_port: None,
                        }),
                        ..Default::default()
                    },
                    action: filter_ext::Action::Accept,
                }
                .into()
            ])
        )
    }

    #[test]
    fn test_rule_with_from_range() {
        assert_eq!(
            parse_str_to_rules(
                "pass in proto tcp from range 10000:10010;",
                &test_filter_routines()
            ),
            Ok(vec![
                filter_ext::Rule {
                    id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                    matchers: filter_ext::Matchers {
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                            src_port: Some(
                                fnet_matchers_ext::Port::new(10000, 10010, false).unwrap()
                            ),
                            dst_port: None,
                        }),
                        ..Default::default()
                    },
                    action: filter_ext::Action::Accept,
                }
                .into()
            ])
        )
    }

    #[test]
    fn test_rule_with_from_invalid_range() {
        assert_eq!(
            parse_str_to_rules(
                "pass in proto tcp from range 10005:10000;",
                &test_filter_routines()
            ),
            Err(Error::Invalid(InvalidReason::InvalidPortRange))
        );
    }

    #[test]
    fn test_rule_with_from_v4_address_port() {
        assert_eq!(
            parse_str_to_rules(
                "pass in proto tcp from 1.2.3.0/24 port 10000;",
                &test_filter_routines()
            ),
            Ok(vec![
                filter_ext::Rule {
                    id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                    matchers: filter_ext::Matchers {
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                            src_port: Some(
                                fnet_matchers_ext::Port::new(10000, 10000, false).unwrap()
                            ),
                            dst_port: None,
                        }),
                        src_addr: Some(fnet_matchers_ext::Address {
                            matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                                fnet_matchers_ext::Subnet::try_from(fidl_subnet!("1.2.3.0/24"))
                                    .unwrap()
                            ),
                            invert: false,
                        }),
                        ..Default::default()
                    },
                    action: filter_ext::Action::Accept,
                }
                .into()
            ])
        )
    }

    #[test]
    fn test_rule_with_from_not_v4_address_port() {
        assert_eq!(
            parse_str_to_rules(
                "pass in proto tcp from !1.2.3.0/24 port 10000;",
                &test_filter_routines()
            ),
            Ok(vec![
                filter_ext::Rule {
                    id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                    matchers: filter_ext::Matchers {
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                            src_port: Some(
                                fnet_matchers_ext::Port::new(10000, 10000, false).unwrap()
                            ),
                            dst_port: None,
                        }),
                        src_addr: Some(fnet_matchers_ext::Address {
                            matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                                fnet_matchers_ext::Subnet::try_from(fidl_subnet!("1.2.3.0/24"))
                                    .unwrap()
                            ),
                            invert: true,
                        }),
                        ..Default::default()
                    },
                    action: filter_ext::Action::Accept,
                }
                .into()
            ])
        )
    }

    #[test]
    fn test_rule_with_from_v6_address_port() {
        assert_eq!(
            parse_str_to_rules(
                "pass in proto tcp from 1234:5678::/32 port 10000;",
                &test_filter_routines()
            ),
            Ok(vec![
                filter_ext::Rule {
                    id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                    matchers: filter_ext::Matchers {
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                            src_port: Some(
                                fnet_matchers_ext::Port::new(10000, 10000, false).unwrap()
                            ),
                            dst_port: None,
                        }),
                        src_addr: Some(fnet_matchers_ext::Address {
                            matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                                fnet_matchers_ext::Subnet::try_from(fidl_subnet!("1234:5678::/32"))
                                    .unwrap()
                            ),
                            invert: false,
                        }),
                        ..Default::default()
                    },
                    action: filter_ext::Action::Accept,
                }
                .into()
            ])
        )
    }

    #[test]
    fn test_rule_with_to_v6_address_port() {
        assert_eq!(
            parse_str_to_rules(
                "pass in proto tcp to 1234:5678::/32 port 10000;",
                &test_filter_routines()
            ),
            Ok(vec![
                filter_ext::Rule {
                    id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                    matchers: filter_ext::Matchers {
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                            src_port: None,
                            dst_port: Some(
                                fnet_matchers_ext::Port::new(10000, 10000, false).unwrap()
                            ),
                        }),
                        dst_addr: Some(fnet_matchers_ext::Address {
                            matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                                fnet_matchers_ext::Subnet::try_from(fidl_subnet!("1234:5678::/32"))
                                    .unwrap()
                            ),
                            invert: false,
                        }),
                        ..Default::default()
                    },
                    action: filter_ext::Action::Accept,
                }
                .into()
            ])
        )
    }

    #[test]
    fn test_rule_with_from_v6_address_port_to_v4_address_port() {
        assert_eq!(
            parse_str_to_rules(
                "pass in proto tcp from 1234:5678::/32 port 10000 to 1.2.3.0/24 port 1000;",
                &test_filter_routines()
            ),
            Err(Error::Invalid(InvalidReason::MixedIPVersions))
        );
    }

    #[test]
    fn test_rule_with_from_v6_address_port_to_v6_address_port() {
        assert_eq!(
            parse_str_to_rules(
                "pass in proto tcp from 1234:5678::/32 port 10000 to 2345:6789::/32 port 1000;",
                &test_filter_routines()
            ),
            Ok(vec![
                filter_ext::Rule {
                    id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                    matchers: filter_ext::Matchers {
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                            src_port: Some(
                                fnet_matchers_ext::Port::new(10000, 10000, false).unwrap()
                            ),
                            dst_port: Some(
                                fnet_matchers_ext::Port::new(1000, 1000, false).unwrap()
                            ),
                        }),
                        src_addr: Some(fnet_matchers_ext::Address {
                            matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                                fnet_matchers_ext::Subnet::try_from(fidl_subnet!("1234:5678::/32"))
                                    .unwrap()
                            ),
                            invert: false,
                        }),
                        dst_addr: Some(fnet_matchers_ext::Address {
                            matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                                fnet_matchers_ext::Subnet::try_from(fidl_subnet!("2345:6789::/32"))
                                    .unwrap()
                            ),
                            invert: false,
                        }),
                        ..Default::default()
                    },
                    action: filter_ext::Action::Accept,
                }
                .into()
            ])
        )
    }

    #[test]
    fn test_rule_with_port_class() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp devclass ap;", &test_filter_routines()),
            Ok(vec![filter_ext::Rule {
                id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                matchers: filter_ext::Matchers {
                    in_interface: Some(fnet_matchers_ext::Interface::PortClass(
                        fnet_interfaces_ext::PortClass::WlanAp,
                    )),
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                        src_port: None,
                        dst_port: None,
                    }),
                    ..Default::default()
                },
                action: filter_ext::Action::Accept,
            }])
        )
    }

    #[test]
    fn test_rule_with_port_class_and_dst_range() {
        assert_eq!(
            parse_str_to_rules(
                "pass in proto tcp devclass ap to range 1:2;",
                &test_filter_routines()
            ),
            Ok(vec![
                filter_ext::Rule {
                    id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                    matchers: filter_ext::Matchers {
                        in_interface: Some(fnet_matchers_ext::Interface::PortClass(
                            fnet_interfaces_ext::PortClass::WlanAp
                        )),
                        transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                            src_port: None,
                            dst_port: Some(fnet_matchers_ext::Port::new(1, 2, false).unwrap()),
                        }),
                        ..Default::default()
                    },
                    action: filter_ext::Action::Accept,
                }
                .into()
            ])
        )
    }

    // Ensure the `log` and `state` fields that are used in `filter_deprecated`
    // can be provided, but have no impact on the parsed rule. These fields
    // have no equivalent in filter2.
    #[test]
    fn test_rule_with_unused_fields() {
        assert_eq!(
            parse_str_to_rules("pass in proto tcp log no state;", &test_filter_routines()),
            Ok(vec![filter_ext::Rule {
                id: filter_ext::RuleId { routine: local_ingress_routine(), index: 0 },
                matchers: filter_ext::Matchers {
                    transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                        src_port: None,
                        dst_port: None,
                    }),
                    ..Default::default()
                },
                action: filter_ext::Action::Accept,
            }])
        )
    }
}
