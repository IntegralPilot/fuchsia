// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! IP routing rules.

use alloc::vec::Vec;
use core::fmt::Debug;
use core::ops::Deref as _;

use net_types::ip::Ip;
use netstack3_base::{
    BoundInterfaceMatcher, InterfaceProperties, MarkMatchers, Marks, Matcher, MatcherBindingsTypes,
    SubnetMatcher,
};

use crate::internal::routing::PacketOrigin;
use crate::{IpRoutingBindingsTypes, RoutingTableId};

/// Table that contains routing rules.
pub struct RulesTable<I: Ip, D, BT: IpRoutingBindingsTypes + MatcherBindingsTypes> {
    /// Rules of the table.
    rules: Vec<Rule<I, D, BT>>,
}

impl<I: Ip, D, BT: IpRoutingBindingsTypes + MatcherBindingsTypes> RulesTable<I, D, BT> {
    pub(crate) fn new(main_table_id: RoutingTableId<I, D, BT>) -> Self {
        // TODO(https://fxbug.dev/355059790): If bindings is installing the main table, we should
        // also let the bindings install this default rule.
        Self {
            rules: alloc::vec![Rule {
                matcher: RuleMatcher::match_all_packets(),
                action: RuleAction::Lookup(main_table_id)
            }],
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &'_ Rule<I, D, BT>> {
        self.rules.iter()
    }

    /// Gets the mutable reference to the rules vector.
    #[cfg(any(test, feature = "testutils"))]
    pub fn rules_mut(&mut self) -> &mut Vec<Rule<I, D, BT>> {
        &mut self.rules
    }

    /// Replaces the rules inside this table.
    pub fn replace(&mut self, new_rules: Vec<Rule<I, D, BT>>) {
        self.rules = new_rules;
    }
}

/// A routing rule.
pub struct Rule<I: Ip, D, BT: IpRoutingBindingsTypes + MatcherBindingsTypes> {
    /// The matcher of the rule.
    pub matcher: RuleMatcher<I, BT::DeviceClass>,
    /// The action of the rule.
    pub action: RuleAction<RoutingTableId<I, D, BT>>,
}

/// The action part of a [`Rule`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleAction<Lookup> {
    /// Will resolve to unreachable.
    Unreachable,
    /// Lookup in a routing table.
    Lookup(Lookup),
}

/// Matches with [`PacketOrigin`].
///
/// Note that this matcher doesn't specify the source address/bound address like [`PacketOrigin`]
/// because the user can specify a source address matcher without specifying the direction of the
/// traffic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrafficOriginMatcher<DeviceClass> {
    /// This only matches packets that are generated locally; the optional interface matcher
    /// can be used to match what device is bound to by `SO_BINDTODEVICE`.
    Local {
        /// The matcher for the bound device.
        // TODO(https://fxbug.dev/441124570): Support referencey semantics for
        // ID matchers.
        bound_device_matcher: Option<BoundInterfaceMatcher<DeviceClass>>,
    },
    /// This only matches non-local packets. The packets must be received from the network.
    NonLocal,
}

impl<'a, I: Ip, D> Matcher<PacketOrigin<I, &'a D>> for SubnetMatcher<I::Addr> {
    fn matches(&self, actual: &PacketOrigin<I, &'a D>) -> bool {
        match actual {
            PacketOrigin::Local { bound_address, bound_device: _ } => {
                self.required_matches(bound_address.as_deref())
            }
            PacketOrigin::NonLocal { source_address, incoming_device: _ } => {
                self.matches(source_address.deref())
            }
        }
    }
}

impl<'a, DeviceClass, I: Ip, D: InterfaceProperties<DeviceClass>> Matcher<PacketOrigin<I, &'a D>>
    for TrafficOriginMatcher<DeviceClass>
{
    fn matches(&self, actual: &PacketOrigin<I, &'a D>) -> bool {
        match (self, actual) {
            (
                TrafficOriginMatcher::Local { bound_device_matcher },
                PacketOrigin::Local { bound_address: _, bound_device },
            ) => bound_device_matcher.matches(bound_device),
            (
                TrafficOriginMatcher::NonLocal,
                PacketOrigin::NonLocal { source_address: _, incoming_device: _ },
            ) => true,
            (TrafficOriginMatcher::Local { .. }, PacketOrigin::NonLocal { .. })
            | (TrafficOriginMatcher::NonLocal, PacketOrigin::Local { .. }) => false,
        }
    }
}

/// Contains traffic matchers for a given rule.
///
/// `None` fields match all packets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleMatcher<I: Ip, DeviceClass> {
    /// Matches on [`PacketOrigin`]'s bound address for a locally generated packet or the source
    /// address of an incoming packet.
    ///
    /// Matches whether the source address of the packet is from the subnet. If the matcher is
    /// specified but the source address is not specified, it resolves to not a match.
    pub source_address_matcher: Option<SubnetMatcher<I::Addr>>,
    /// Matches on [`PacketOrigin`]'s bound device for a locally generated packets or the receiving
    /// device of an incoming packet.
    pub traffic_origin_matcher: Option<TrafficOriginMatcher<DeviceClass>>,
    /// Matches on [`RuleInput`]'s marks.
    pub mark_matchers: MarkMatchers,
}

impl<I: Ip, DeviceClass> RuleMatcher<I, DeviceClass> {
    /// Creates a rule matcher that matches all packets.
    pub fn match_all_packets() -> Self {
        RuleMatcher {
            source_address_matcher: None,
            traffic_origin_matcher: None,
            mark_matchers: MarkMatchers::default(),
        }
    }
}

/// Packet properties used as input for the rules engine.
pub struct RuleInput<'a, I: Ip, D> {
    pub(crate) packet_origin: PacketOrigin<I, &'a D>,
    pub(crate) marks: &'a Marks,
}

impl<'a, I: Ip, D: InterfaceProperties<DeviceClass>, DeviceClass> Matcher<RuleInput<'a, I, D>>
    for RuleMatcher<I, DeviceClass>
{
    fn matches(&self, actual: &RuleInput<'a, I, D>) -> bool {
        let Self { source_address_matcher, traffic_origin_matcher, mark_matchers } = self;
        let RuleInput { packet_origin, marks } = actual;
        source_address_matcher.matches(packet_origin)
            && traffic_origin_matcher.matches(packet_origin)
            && mark_matchers.matches(marks)
    }
}

#[cfg(test)]
mod test {
    use ip_test_macro::ip_test;
    use net_types::SpecifiedAddr;
    use net_types::ip::Subnet;
    use netstack3_base::InterfaceMatcher;
    use netstack3_base::testutil::{FakeDeviceId, MultipleDevicesId, TestIpExt};
    use test_case::test_case;

    use super::*;

    #[ip_test(I)]
    #[test_case(None, None => true)]
    #[test_case(None, Some(MultipleDevicesId::A) => true)]
    #[test_case(
        Some(BoundInterfaceMatcher::Unbound),
        None => true)]
    #[test_case(
        Some(BoundInterfaceMatcher::Unbound),
        Some(MultipleDevicesId::A) => false)]
    #[test_case(
        Some(BoundInterfaceMatcher::Bound(InterfaceMatcher::Name("A".into()))),
        None => false)]
    #[test_case(
        Some(BoundInterfaceMatcher::Bound(InterfaceMatcher::Name("A".into()))),
        Some(MultipleDevicesId::A) => true)]
    #[test_case(
        Some(BoundInterfaceMatcher::Bound(InterfaceMatcher::Name("A".into()))),
        Some(MultipleDevicesId::B) => false)]
    fn rule_matcher_matches_bound_device<I: TestIpExt>(
        bound_device_matcher: Option<BoundInterfaceMatcher<()>>,
        bound_device: Option<MultipleDevicesId>,
    ) -> bool {
        let matcher = RuleMatcher::<I, ()> {
            traffic_origin_matcher: Some(TrafficOriginMatcher::Local { bound_device_matcher }),
            ..RuleMatcher::match_all_packets()
        };
        let input = RuleInput {
            packet_origin: PacketOrigin::Local {
                bound_address: None,
                bound_device: bound_device.as_ref(),
            },
            marks: &Default::default(),
        };
        matcher.matches(&input)
    }

    #[ip_test(I)]
    #[test_case(None, None => true)]
    #[test_case(None, Some(I::LOOPBACK_ADDRESS) => true)]
    #[test_case(
        Some(<I as TestIpExt>::TEST_ADDRS.subnet),
        None => false)]
    #[test_case(
        Some(<I as TestIpExt>::TEST_ADDRS.subnet),
        Some(<I as TestIpExt>::TEST_ADDRS.local_ip) => true)]
    #[test_case(
        Some(<I as TestIpExt>::TEST_ADDRS.subnet),
        Some(<I as TestIpExt>::get_other_remote_ip_address(1)) => false)]
    fn rule_matcher_matches_local_addr<I: TestIpExt>(
        source_address_subnet: Option<Subnet<I::Addr>>,
        bound_address: Option<SpecifiedAddr<I::Addr>>,
    ) -> bool {
        let matcher = RuleMatcher::<I, ()> {
            source_address_matcher: source_address_subnet.map(SubnetMatcher),
            ..RuleMatcher::match_all_packets()
        };
        let marks = Default::default();
        let input = RuleInput::<'_, _, FakeDeviceId> {
            packet_origin: PacketOrigin::Local { bound_address, bound_device: None },
            marks: &marks,
        };
        matcher.matches(&input)
    }

    #[ip_test(I)]
    #[test_case(None, PacketOrigin::Local {
         bound_address: None,
         bound_device: None
    } => true)]
    #[test_case(None, PacketOrigin::NonLocal {
        source_address: <I as TestIpExt>::TEST_ADDRS.remote_ip,
        incoming_device: &FakeDeviceId
    } => true)]
    #[test_case(Some(TrafficOriginMatcher::Local {
        bound_device_matcher: None
    }), PacketOrigin::Local {
        bound_address: None,
        bound_device: None
    } => true)]
    #[test_case(Some(TrafficOriginMatcher::NonLocal),
        PacketOrigin::NonLocal {
            source_address: <I as TestIpExt>::TEST_ADDRS.remote_ip,
            incoming_device: &FakeDeviceId
        } => true)]
    #[test_case(Some(TrafficOriginMatcher::Local { bound_device_matcher: None }),
        PacketOrigin::NonLocal {
            source_address: <I as TestIpExt>::TEST_ADDRS.remote_ip,
            incoming_device: &FakeDeviceId
        }  => false)]
    #[test_case(Some(TrafficOriginMatcher::NonLocal),
        PacketOrigin::Local {
            bound_address: None,
            bound_device: None
        } => false)]
    fn rule_matcher_matches_locally_generated<I: TestIpExt>(
        traffic_origin_matcher: Option<TrafficOriginMatcher<()>>,
        packet_origin: PacketOrigin<I, &'static FakeDeviceId>,
    ) -> bool {
        let matcher =
            RuleMatcher::<I, ()> { traffic_origin_matcher, ..RuleMatcher::match_all_packets() };
        let marks = Default::default();
        let input = RuleInput::<'_, _, FakeDeviceId> { packet_origin, marks: &marks };
        matcher.matches(&input)
    }

    #[ip_test(I, test = false)]
    #[test_case::test_matrix(
            [
                None,
                Some(<I as TestIpExt>::TEST_ADDRS.local_ip),
                Some(<I as TestIpExt>::get_other_remote_ip_address(1))
            ],
            [
                None,
                Some(&MultipleDevicesId::A),
                Some(&MultipleDevicesId::B),
                Some(&MultipleDevicesId::C),
            ],
            [true, false]
        )]
    fn rule_matcher_matches_multiple_conditions<I: TestIpExt>(
        ip: Option<SpecifiedAddr<I::Addr>>,
        device: Option<&'static MultipleDevicesId>,
        locally_generated: bool,
    ) {
        let matcher = RuleMatcher::<I, ()> {
            source_address_matcher: Some(SubnetMatcher(I::TEST_ADDRS.subnet)),
            traffic_origin_matcher: Some(TrafficOriginMatcher::Local {
                bound_device_matcher: Some(BoundInterfaceMatcher::Bound(InterfaceMatcher::Name(
                    "A".into(),
                ))),
            }),
            ..RuleMatcher::match_all_packets()
        };

        let packet_origin = if locally_generated {
            PacketOrigin::Local { bound_address: ip, bound_device: device }
        } else {
            let (Some(source_address), Some(incoming_device)) = (ip, device) else {
                return;
            };
            PacketOrigin::NonLocal { source_address, incoming_device }
        };

        let input = RuleInput { packet_origin, marks: &Default::default() };

        if ip == Some(I::TEST_ADDRS.local_ip)
            && (device == Some(&MultipleDevicesId::A))
            && locally_generated
        {
            assert!(matcher.matches(&input))
        } else {
            assert!(!matcher.matches(&input))
        }
    }
}
