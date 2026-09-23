// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::BTreeMap;

use linux_uapi::rt_class_t_RT_TABLE_COMPAT as RT_TABLE_COMPAT;
use net_types::ip::{GenericOverIp, Ip, IpVersion};
use netlink_packet_route::AddressFamily;
use netlink_packet_route::rule::{
    RuleAction, RuleAttribute, RuleHeader, RuleMessage, RuleUidRange,
};

use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;

use crate::route_tables::NetlinkRouteTableIndex;

#[derive(GenericOverIp, Debug, PartialEq, Eq)]
#[generic_over_ip(I, Ip)]
pub(super) struct FidlRule<I: Ip> {
    pub(super) index: fnet_routes_ext::rules::RuleIndex,
    pub(super) matcher: fnet_routes_ext::rules::RuleMatcher<I>,
    pub(super) action: Action,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Action {
    Unreachable,
    ToTable(NetlinkRouteTableIndex),
}

#[derive(Debug)]
pub(super) struct RuleIndexOverflowError;

/// Computes a [`fnet_routes_ext::rules::RuleIndex`] encoding a netlink rule `priority` as well as
/// the index within that priority set (reflecting the order in which rules were installed).
///
/// Because the fall-through main-table lookup rule is at `u16::MAX`, we do not expect ever to need
/// to specify a priority higher than this, thus allowing us to fit everything in one u32.
// In the future, we could consider either widening [`fnet_routes_ext::rules::RuleIndex`] to a u64,
// or using a separate RuleSet for each priority.
pub(super) fn compute_rule_index(
    priority: u32,
    index_within_priority: u16,
) -> Result<fnet_routes_ext::rules::RuleIndex, RuleIndexOverflowError> {
    let max_allowed_priority = u32::from(u16::MAX);
    if priority > max_allowed_priority {
        return Err(RuleIndexOverflowError);
    }
    Ok((priority << 16 | u32::from(index_within_priority)).into())
}

#[derive(Debug, Copy, Clone, GenericOverIp, PartialEq, Eq)]
#[generic_over_ip()]
pub(super) enum FidlRuleConversionError {
    IpVersionMismatch,
    UnsupportedAction,
    InvalidTable,
    InvalidSourceSubnet,
    RuleIndexOverflow,
    NoSuchInterface,
}

struct RuleAttributes<'a> {
    priority: Option<u32>,
    action: Action,
    oifname: Option<&'a str>,
    iifname: Option<&'a str>,
    source: Option<std::net::IpAddr>,
    fwmark: Option<u32>,
    fwmask: Option<u32>,
    uid_range: Option<RuleUidRange>,
}

fn extract_attributes(
    rule_message: &RuleMessage,
) -> Result<RuleAttributes<'_>, FidlRuleConversionError> {
    let mut priority: Option<u32> = None;
    let mut table: Option<u32> = None;
    let mut oifname: Option<&str> = None;
    let mut iifname: Option<&str> = None;
    let mut source: Option<std::net::IpAddr> = None;
    let mut fwmark: Option<u32> = None;
    let mut fwmask: Option<u32> = None;
    let mut uid_range: Option<RuleUidRange> = None;

    for attribute in &rule_message.attributes {
        match attribute {
            RuleAttribute::Priority(priority_attr) => {
                priority = Some(*priority_attr);
            }
            RuleAttribute::Table(table_attr) => {
                table = Some(*table_attr);
            }
            RuleAttribute::Oifname(oifname_attr) => {
                oifname = Some(oifname_attr);
            }
            RuleAttribute::Iifname(iifname_attr) => {
                iifname = Some(iifname_attr);
            }
            RuleAttribute::Source(source_attr) => {
                source = Some(*source_attr);
            }
            RuleAttribute::FwMark(fwmark_attr) => {
                fwmark = Some(*fwmark_attr);
            }
            RuleAttribute::FwMask(fwmask_attr) => {
                fwmask = Some(*fwmask_attr);
            }
            RuleAttribute::UidRange(uid_range_attr) => {
                uid_range = Some(*uid_range_attr);
            }
            _ => {}
        }
    }
    let table = match (rule_message.header.table as u32, table) {
        // The "COMPAT" table indicates that the table attribute should be set and used for the
        // table.
        (RT_TABLE_COMPAT, None) => Err(FidlRuleConversionError::InvalidTable),
        (RT_TABLE_COMPAT, Some(attr)) => Ok(NetlinkRouteTableIndex::new(attr)),
        // Otherwise, if they're both set, they should agree.
        (other, Some(attr)) => (other == attr || other == 0)
            .then_some(NetlinkRouteTableIndex::new(attr))
            .ok_or(FidlRuleConversionError::InvalidTable),
        (other, None) => Ok(NetlinkRouteTableIndex::new(u32::from(other))),
    };

    Ok(RuleAttributes {
        priority,
        action: match rule_message.header.action {
            RuleAction::Unreachable => Action::Unreachable,
            // We (arbitrarily) choose to be permissive and only return an error for an invalid
            // table if the table is actually needed by the action.
            RuleAction::ToTable => table.map(Action::ToTable)?,
            _ => return Err(FidlRuleConversionError::UnsupportedAction),
        },
        oifname,
        iifname,
        source,
        fwmark,
        fwmask,
        uid_range,
    })
}

/// Trait abstracting the ability to check if an interface is loopback.
pub(crate) trait LookupIfInterfaceIsLoopback {
    /// Returns `None` if the interface does not exist, `Some(true)` if the interface exists and is
    /// a loopback interface, `Some(false)` if the interface exists and is not a loopback interface.
    fn is_loopback(&self, interface_name: &str) -> Option<bool>;
}

impl LookupIfInterfaceIsLoopback
    for BTreeMap<
        u64,
        fnet_interfaces_ext::PropertiesAndState<
            crate::interfaces::InterfaceState,
            fnet_interfaces_ext::AllInterest,
        >,
    >
{
    fn is_loopback(&self, interface_name: &str) -> Option<bool> {
        self.values().find_map(|fnet_interfaces_ext::PropertiesAndState { properties, .. }| {
            let fnet_interfaces_ext::Properties { name, port_class, .. } = properties;
            (name == interface_name)
                .then_some(matches!(port_class, fnet_interfaces_ext::PortClass::Loopback))
        })
    }
}

pub(super) fn fidl_rule_from_rule_message<I: Ip>(
    rule_message: &RuleMessage,
    current_index: u16,
    interface_properties: &impl LookupIfInterfaceIsLoopback,
) -> Result<FidlRule<I>, FidlRuleConversionError> {
    let RuleAttributes { priority, action, oifname, iifname, source, fwmark, fwmask, uid_range } =
        extract_attributes(rule_message)?;

    let RuleMessage {
        header: RuleHeader { family, dst_len: _, src_len, tos: _, table: _, action: _, flags: _ },
        ..
    } = rule_message;

    match (I::VERSION, family) {
        (IpVersion::V4, AddressFamily::Inet) | (IpVersion::V6, AddressFamily::Inet6) => (),
        _ => {
            return Err(FidlRuleConversionError::IpVersionMismatch);
        }
    }

    // The intended behavior according to `ip-rule` docs is that if the `iif` specified is loopback,
    // then the rule only matches packets originating from this host. If the `iif` specified is a
    // non-loopback interface, then the rule only matches packets we're forwarding that came in on
    // that interface. If when we look up the interface, we find it doesn't exist, the best thing we
    // can do is require `locally_generated` to be false since the intent was to only match on
    // forwarded traffic.
    let locally_generated = match iifname {
        None => None,
        Some(name) => match interface_properties.is_loopback(name) {
            None => {
                crate::logging::log_warn!(
                    "rule {rule_message:?} contains nonexistent iifname {name}"
                );
                return Err(FidlRuleConversionError::NoSuchInterface);
            }
            Some(is_loopback) => Some(is_loopback),
        },
    };
    let bound_device = oifname
        .map(|name| {
            fnet_matchers_ext::BoundInterface::Bound(fnet_matchers_ext::Interface::Name(
                name.to_owned(),
            ))
        })
        .or_else(||
            // If the action is `Unreachable`, we want to allow traffic with SO_BINDTODEVICE to
            // sidestep the unreachable rule and look into the main table. In most cases, Linux
            // would simply ignore the unreachable rule if the socket is bound to a device. The
            // motivating use case is when an application does SO_BINDTODEVICE and wants to use the
            // subnet route that only exists in the main table.
            //
            // See https://fxbug.dev/404262204 for more details.
            (action == Action::Unreachable)
                .then_some(fnet_matchers_ext::BoundInterface::Unbound));

    let mark_1 = fwmark.map(|fwmark| fnet_matchers_ext::Mark::Marked {
        // If no mask is specified, default to checking the entire mark.
        mask: fwmask.unwrap_or(!0),
        between: fwmark..=fwmark,
        invert: false,
    });

    // Fuchsia doesn't have a concept of a `uid`, so netlink and starnix have to agree on using
    // `mark_2` to encode this.
    let mark_2 = uid_range.map(|RuleUidRange { start, end }| fnet_matchers_ext::Mark::Marked {
        mask: !0,
        between: start..=end,
        invert: false,
    });

    let priority = priority.unwrap_or_else(|| {
        crate::logging::log_warn!(
            "RuleMessage {rule_message:?} did not specify priority, defaulting to 0"
        );
        0
    });

    let index = compute_rule_index(priority, current_index)
        .map_err(|RuleIndexOverflowError| FidlRuleConversionError::RuleIndexOverflow)?;

    pub(super) fn try_from_source_ip_addr<I: Ip>(
        source: std::net::IpAddr,
        src_len: u8,
    ) -> Result<net_types::ip::Subnet<I::Addr>, FidlRuleConversionError> {
        I::map_ip_out(
            source,
            |source| match source {
                std::net::IpAddr::V4(source) => net_types::ip::Subnet::new(
                    net_types::ip::Ipv4Addr::new(source.octets()),
                    src_len,
                )
                .map_err(|_| FidlRuleConversionError::InvalidSourceSubnet),
                std::net::IpAddr::V6(_source) => Err(FidlRuleConversionError::IpVersionMismatch),
            },
            |source| match source {
                std::net::IpAddr::V6(source) => net_types::ip::Subnet::new(
                    net_types::ip::Ipv6Addr::from_bytes(source.octets()),
                    src_len,
                )
                .map_err(|_| FidlRuleConversionError::InvalidSourceSubnet),
                std::net::IpAddr::V4(_source) => Err(FidlRuleConversionError::IpVersionMismatch),
            },
        )
    }

    Ok(FidlRule {
        index,
        matcher: fnet_routes_ext::rules::RuleMatcher {
            from: match source {
                None => None,
                Some(source) => Some(try_from_source_ip_addr::<I>(source, *src_len)?),
            },
            locally_generated,
            bound_device,
            mark_1,
            mark_2,
        },
        action,
    })
}

pub(super) fn get_referenced_table(rule_message: &RuleMessage) -> Option<NetlinkRouteTableIndex> {
    match extract_attributes(rule_message).ok()?.action {
        Action::Unreachable => None,
        Action::ToTable(table) => Some(table),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use std::collections::BTreeMap;
    use std::str::FromStr as _;

    use ip_test_macro::ip_test;
    use linux_uapi::{rt_class_t_RT_TABLE_COMPAT, rt_class_t_RT_TABLE_MAIN};
    use net_types::ip::{Ip, IpVersion, IpVersionMarker, Ipv4, Ipv6};
    use netlink_packet_route::AddressFamily;
    use netlink_packet_route::rule::{
        RuleAction, RuleAttribute, RuleFlags, RuleHeader, RuleMessage, RuleUidRange,
    };
    use test_case::test_case;

    use fidl_fuchsia_net_routes_ext::rules::{RuleIndex, RuleMatcher};

    use crate::route_tables::NetlinkRouteTableIndex;

    #[derive(Debug)]
    struct RuleArgs {
        family: AddressFamily,
        table_in_header: u8,
        action: RuleAction,
        rule_attributes: Vec<RuleAttribute>,
        src_len: u8,
    }

    fn create_rule_message(
        RuleArgs { family, table_in_header, action, rule_attributes, src_len }: RuleArgs,
    ) -> RuleMessage {
        let mut rule_message = RuleMessage::default();
        rule_message.header = RuleHeader {
            family,
            dst_len: 0,
            src_len,
            tos: 0,
            table: table_in_header,
            action,
            flags: RuleFlags::empty(),
        };
        rule_message.attributes.extend(rule_attributes);
        rule_message
    }

    fn address_family<I: Ip>() -> AddressFamily {
        match I::VERSION {
            IpVersion::V4 => AddressFamily::Inet,
            IpVersion::V6 => AddressFamily::Inet6,
        }
    }

    fn basic_rule_args<I: Ip>() -> RuleArgs {
        RuleArgs {
            family: address_family::<I>(),
            table_in_header: rt_class_t_RT_TABLE_MAIN as u8,
            action: RuleAction::Unreachable,
            rule_attributes: Vec::new(),
            src_len: 0,
        }
    }

    #[derive(Debug)]
    struct LoIsLoopback {
        other_interface_names: Vec<&'static str>,
    }

    impl Default for LoIsLoopback {
        fn default() -> Self {
            Self { other_interface_names: Vec::default() }
        }
    }

    impl LookupIfInterfaceIsLoopback for LoIsLoopback {
        fn is_loopback(&self, name: &str) -> Option<bool> {
            if name == "lo" {
                return Some(true);
            }
            self.other_interface_names.iter().any(|s| *s == name).then_some(false)
        }
    }

    fn test_convert_rule_args<I: Ip>(
        args: RuleArgs,
    ) -> Result<FidlRule<I>, FidlRuleConversionError> {
        fidl_rule_from_rule_message::<I>(&create_rule_message(args), 0, &BTreeMap::new())
    }

    #[ip_test(I)]
    fn converts_basic_test_rule<I: Ip>() {
        assert_eq!(
            test_convert_rule_args::<I>(basic_rule_args::<I>()),
            Ok(FidlRule {
                index: RuleIndex::new(0),
                matcher: RuleMatcher {
                    from: None,
                    locally_generated: None,
                    bound_device: Some(fnet_matchers_ext::BoundInterface::Unbound),
                    mark_1: None,
                    mark_2: None,
                },
                action: Action::Unreachable
            })
        );
    }

    #[test]
    fn rejects_wrong_family() {
        assert_eq!(
            test_convert_rule_args::<Ipv4>(basic_rule_args::<Ipv6>()),
            Err(FidlRuleConversionError::IpVersionMismatch)
        );
        assert_eq!(
            test_convert_rule_args::<Ipv6>(basic_rule_args::<Ipv4>()),
            Err(FidlRuleConversionError::IpVersionMismatch)
        );
    }

    #[ip_test(I)]
    fn rejects_unsupported_action<I: Ip>() {
        assert_eq!(
            test_convert_rule_args::<I>(RuleArgs {
                action: RuleAction::Blackhole,
                ..basic_rule_args::<I>()
            }),
            Err(FidlRuleConversionError::UnsupportedAction)
        )
    }

    #[ip_test(I)]
    #[test_case(rt_class_t_RT_TABLE_COMPAT as u8, None
                => matches Err(FidlRuleConversionError::InvalidTable))]
    #[test_case(rt_class_t_RT_TABLE_MAIN as u8, Some(rt_class_t_RT_TABLE_MAIN + 1)
                => matches Err(FidlRuleConversionError::InvalidTable))]
    #[test_case(rt_class_t_RT_TABLE_COMPAT as u8, Some(rt_class_t_RT_TABLE_MAIN)
                => matches Ok(_))]
    #[test_case(rt_class_t_RT_TABLE_COMPAT as u8, Some(0)
                => matches Ok(_))]
    #[test_case(0, Some(rt_class_t_RT_TABLE_MAIN) => matches Ok(_))]
    fn rejects_invalid_table_when_used<I: Ip>(
        table_in_header: u8,
        table_attribute: Option<u32>,
    ) -> Result<(), FidlRuleConversionError> {
        test_convert_rule_args::<I>(RuleArgs {
            table_in_header,
            action: RuleAction::ToTable,
            rule_attributes: table_attribute
                .map(RuleAttribute::Table)
                .into_iter()
                .collect::<Vec<_>>(),
            ..basic_rule_args::<I>()
        })
        .map(|_| ())
    }

    #[ip_test(I)]
    #[test_case(rt_class_t_RT_TABLE_COMPAT as u8, None
                => matches Ok(_))]
    #[test_case(rt_class_t_RT_TABLE_MAIN as u8, Some(rt_class_t_RT_TABLE_MAIN + 1)
                => matches Ok(_))]
    #[test_case(rt_class_t_RT_TABLE_COMPAT as u8, Some(rt_class_t_RT_TABLE_MAIN)
                => matches Ok(_))]
    #[test_case(rt_class_t_RT_TABLE_COMPAT as u8, Some(0)
                => matches Ok(_))]
    fn accepts_invalid_table_when_unused<I: Ip>(
        table_in_header: u8,
        table_attribute: Option<u32>,
    ) -> Result<(), FidlRuleConversionError> {
        test_convert_rule_args::<I>(RuleArgs {
            table_in_header,
            action: RuleAction::Unreachable,
            rule_attributes: table_attribute
                .map(RuleAttribute::Table)
                .into_iter()
                .collect::<Vec<_>>(),
            ..basic_rule_args::<I>()
        })
        .map(|_| ())
    }

    #[ip_test(I)]
    // Includes valid tables when they're used
    #[test_case(rt_class_t_RT_TABLE_COMPAT as u8,
        Some(rt_class_t_RT_TABLE_MAIN), RuleAction::ToTable => Some(rt_class_t_RT_TABLE_MAIN))]
    #[test_case(rt_class_t_RT_TABLE_COMPAT as u8,
        Some(0), RuleAction::ToTable => Some(0))]
    // Doesn't count tables when they're invalid
    #[test_case(rt_class_t_RT_TABLE_COMPAT as u8,
        None, RuleAction::ToTable => None)]
    #[test_case(rt_class_t_RT_TABLE_MAIN as u8,
        Some(rt_class_t_RT_TABLE_MAIN + 1), RuleAction::ToTable => None)]
    // Doesn't count tables when the action doesn't use them
    #[test_case(rt_class_t_RT_TABLE_COMPAT as u8,
        None, RuleAction::Unreachable => None)]
    #[test_case(rt_class_t_RT_TABLE_MAIN as u8,
        Some(rt_class_t_RT_TABLE_MAIN + 1), RuleAction::Unreachable => None)]
    #[test_case(rt_class_t_RT_TABLE_COMPAT as u8,
        Some(rt_class_t_RT_TABLE_MAIN), RuleAction::Unreachable => None)]
    #[test_case(rt_class_t_RT_TABLE_COMPAT as u8,
        Some(0), RuleAction::Unreachable => None)]
    fn get_referenced_table_agrees_with_converter<I: Ip>(
        table_in_header: u8,
        table_attribute: Option<u32>,
        action: RuleAction,
    ) -> Option<u32> {
        let rule_message = create_rule_message(RuleArgs {
            table_in_header,
            action,
            rule_attributes: table_attribute
                .map(RuleAttribute::Table)
                .into_iter()
                .collect::<Vec<_>>(),
            ..basic_rule_args::<I>()
        });
        get_referenced_table(&rule_message).map(|index: NetlinkRouteTableIndex| index.get())
    }

    #[ip_test(I)]
    #[test_case(Some("lo") => Ok(Some(true)))]
    #[test_case(Some("not-lo") => Ok(Some(false)))]
    #[test_case(Some("nonexistent-if") => Err(FidlRuleConversionError::NoSuchInterface))]
    #[test_case(None => Ok(None))]
    fn locally_generated<I: Ip>(
        iifname: Option<&str>,
    ) -> Result<Option<bool>, FidlRuleConversionError> {
        let args = RuleArgs {
            rule_attributes: iifname
                .map(|name| RuleAttribute::Iifname(name.to_owned()))
                .into_iter()
                .collect::<Vec<_>>(),
            ..basic_rule_args::<I>()
        };

        fidl_rule_from_rule_message::<I>(
            &create_rule_message(args),
            0,
            &LoIsLoopback { other_interface_names: vec!["not-lo"] },
        )
        .map(|fidl_rule| fidl_rule.matcher.locally_generated)
    }

    #[ip_test(I)]
    #[test_case(Some("device-name"), RuleAction::Unreachable
        => Some(fnet_matchers_ext::BoundInterface::Bound(fnet_matchers_ext::Interface::Name("device-name".to_owned()))))]
    #[test_case(Some("device-name"), RuleAction::ToTable
        => Some(fnet_matchers_ext::BoundInterface::Bound(fnet_matchers_ext::Interface::Name("device-name".to_owned()))))]
    #[test_case(None, RuleAction::Unreachable => Some(fnet_matchers_ext::BoundInterface::Unbound))]
    #[test_case(None, RuleAction::ToTable => None)]
    fn bound_device<I: Ip>(
        oifname: Option<&str>,
        action: RuleAction,
    ) -> Option<fnet_matchers_ext::BoundInterface> {
        test_convert_rule_args::<I>(RuleArgs {
            rule_attributes: oifname
                .map(|name| RuleAttribute::Oifname(name.to_owned()))
                .into_iter()
                .collect::<Vec<_>>(),
            action,
            table_in_header: rt_class_t_RT_TABLE_MAIN as u8,
            ..basic_rule_args::<I>()
        })
        .expect("conversion should succeed")
        .matcher
        .bound_device
    }

    #[ip_test(I)]
    #[test_case(Some(1234), Some(5678) => Some(fnet_matchers_ext::Mark::Marked {
        mask: 5678,
        between: 1234..=1234,
        invert: false,
    }))]
    #[test_case(None, Some(5678) => None;
        "defaults to checking mark is set (if mark is unset and mask is set)")]
    #[test_case(None, None => None;
        "defaults to checking mark is set (if mark is unset and mask is unset)")]
    fn mark_1_is_fwmark<I: Ip>(
        fwmark: Option<u32>,
        fwmask: Option<u32>,
    ) -> Option<fnet_matchers_ext::Mark> {
        test_convert_rule_args::<I>(RuleArgs {
            rule_attributes: [fwmark.map(RuleAttribute::FwMark), fwmask.map(RuleAttribute::FwMask)]
                .into_iter()
                .filter_map(std::convert::identity)
                .collect::<Vec<_>>(),
            ..basic_rule_args::<I>()
        })
        .expect("conversion should succeed")
        .matcher
        .mark_1
    }

    #[ip_test(I)]
    #[test_case(Some(RuleUidRange { start: 1, end: 10 })
                => Some(fnet_matchers_ext::Mark::Marked {
                    mask: u32::MAX,
                    between: 1..=10,
                    invert: false,
    }))]
    #[test_case(None => None)]
    fn mark_2_is_uid<I: Ip>(uidrange: Option<RuleUidRange>) -> Option<fnet_matchers_ext::Mark> {
        test_convert_rule_args::<I>(RuleArgs {
            rule_attributes: uidrange
                .map(|uidrange| RuleAttribute::UidRange(uidrange))
                .into_iter()
                .collect::<Vec<_>>(),
            ..basic_rule_args::<I>()
        })
        .expect("conversion should succeed")
        .matcher
        .mark_2
    }

    #[ip_test(I)]
    #[test_case(Some(0), 0 => 0)]
    #[test_case(None, 0 => 0)]
    #[test_case(None, 1 => 1)]
    #[test_case(Some(0), 1 => 1)]
    #[test_case(Some(1 << 15), 0 => 1 << 31)]
    #[test_case(Some(1 << 15), 1 => 0b10000000000000000000000000000001)]
    fn rule_index<I: Ip>(priority_attribute: Option<u32>, index_within_priority: u16) -> u32 {
        let rule_message = create_rule_message(RuleArgs {
            rule_attributes: priority_attribute
                .map(|priority| RuleAttribute::Priority(priority))
                .into_iter()
                .collect::<Vec<_>>(),
            ..basic_rule_args::<I>()
        });
        let fidl_rule = fidl_rule_from_rule_message::<I>(
            &rule_message,
            index_within_priority,
            &LoIsLoopback::default(),
        )
        .expect("conversion should succeed");
        fidl_rule.index.into()
    }

    #[ip_test(I)]
    fn rule_index_overflows_if_priority_at_least_2_to_the_16<I: Ip>() {
        let rule_message = create_rule_message(RuleArgs {
            rule_attributes: vec![RuleAttribute::Priority(1 << 16)],
            ..basic_rule_args::<I>()
        });
        let result = fidl_rule_from_rule_message::<I>(&rule_message, 0, &LoIsLoopback::default());
        assert_eq!(result, Err(FidlRuleConversionError::RuleIndexOverflow));
    }

    const V4: IpVersionMarker<Ipv4> = <Ipv4 as Ip>::VERSION_MARKER;
    const V6: IpVersionMarker<Ipv6> = <Ipv6 as Ip>::VERSION_MARKER;

    #[test_case(V4, Some("192.0.2.1"), 32 => Ok(Some("192.0.2.1/32".to_owned())) )]
    #[test_case(V4, Some("192.0.2.0"), 31 => Ok(Some("192.0.2.0/31".to_owned())) )]
    #[test_case(V6, Some("192.0.2.1"), 32 => Err(FidlRuleConversionError::IpVersionMismatch))]
    #[test_case(V4, Some("192.0.2.1"), 31 => Err(FidlRuleConversionError::InvalidSourceSubnet))]
    #[test_case(V4, None, 32 => Ok(None))]
    #[test_case(V4, None, 0 => Ok(None))]
    #[test_case(V6, Some("2001:db8::1"), 128 => Ok(Some("2001:db8::1/128".to_owned())) )]
    #[test_case(V6, Some("2001:db8::"), 127 => Ok(Some("2001:db8::/127".to_owned())) )]
    #[test_case(V4, Some("2001:db8::1"), 127 => Err(FidlRuleConversionError::IpVersionMismatch))]
    #[test_case(V6, Some("2001:db8::1"), 127 => Err(FidlRuleConversionError::InvalidSourceSubnet))]
    #[test_case(V6, None, 128 => Ok(None))]
    #[test_case(V6, None, 0 => Ok(None))]
    fn from_matcher<I: Ip>(
        _intended_ip_version: IpVersionMarker<I>,
        source: Option<&'static str>,
        src_len: u8,
    ) -> Result<Option<String>, FidlRuleConversionError> {
        Ok(test_convert_rule_args::<I>(RuleArgs {
            rule_attributes: source
                .map(|source| {
                    RuleAttribute::Source(
                        std::net::IpAddr::from_str(source).expect("should parse successfully"),
                    )
                })
                .into_iter()
                .collect::<Vec<_>>(),
            src_len,
            ..basic_rule_args::<I>()
        })?
        .matcher
        .from
        .map(|from: net_types::ip::Subnet<I::Addr>| from.to_string()))
    }

    #[ip_test(I)]
    fn supports_all_intended_attributes<I: Ip>() {
        let mut rule_message = create_rule_message(RuleArgs {
            table_in_header: rt_class_t_RT_TABLE_COMPAT as u8,
            action: RuleAction::ToTable,
            rule_attributes: vec![
                RuleAttribute::Priority(1),
                RuleAttribute::Table(5678),
                RuleAttribute::Oifname("eth0".to_owned()),
                RuleAttribute::Iifname("lo".to_owned()),
                RuleAttribute::Source(match I::VERSION {
                    IpVersion::V4 => std::net::IpAddr::from_str("192.0.2.1").unwrap(),
                    IpVersion::V6 => std::net::IpAddr::from_str("2001:db8::1").unwrap(),
                }),
                RuleAttribute::FwMark(0xDEADBEEF),
                RuleAttribute::FwMask(0xBEEFDEAD),
                RuleAttribute::UidRange(RuleUidRange { start: 42, end: 60 }),
            ],
            ..basic_rule_args::<I>()
        });
        rule_message.header.src_len = match I::VERSION {
            IpVersion::V4 => 32,
            IpVersion::V6 => 128,
        };

        let expected_source_subnet = I::map_ip_out(
            (),
            |()| net_declare::net_subnet_v4!("192.0.2.1/32"),
            |()| net_declare::net_subnet_v6!("2001:db8::1/128"),
        );

        assert_eq!(
            fidl_rule_from_rule_message::<I>(&rule_message, 0, &LoIsLoopback::default()),
            Ok(FidlRule {
                index: RuleIndex::new(65536),
                matcher: RuleMatcher {
                    from: Some(expected_source_subnet),
                    locally_generated: Some(true),
                    bound_device: Some(fnet_matchers_ext::BoundInterface::Bound(
                        fnet_matchers_ext::Interface::Name("eth0".to_owned())
                    )),
                    mark_1: Some(fnet_matchers_ext::Mark::Marked {
                        mask: 0xBEEFDEAD,
                        between: 0xDEADBEEF..=0xDEADBEEF,
                        invert: false,
                    }),
                    mark_2: Some(fnet_matchers_ext::Mark::Marked {
                        mask: u32::MAX,
                        between: 42..=60,
                        invert: false,
                    })
                },
                action: Action::ToTable(NetlinkRouteTableIndex::new(5678))
            })
        );
    }
}
