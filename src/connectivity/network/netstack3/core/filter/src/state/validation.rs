// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::Debug;

use assert_matches::assert_matches;
use derivative::Derivative;
use net_types::ip::{GenericOverIp, Ip};
use netstack3_base::MatcherBindingsTypes;
use netstack3_hashmap::hash_map::{Entry, HashMap};
use packet_formats::ip::{IpExt, IpProto, Ipv4Proto, Ipv6Proto};

use crate::{
    Action, Hook, IpRoutines, NatRoutines, PacketMatcher, RejectType, Routine, Routines, Rule,
    TransportProtocolMatcher, UninstalledRoutine,
};

/// Provided filtering state was invalid.
#[derive(Derivative, Debug, GenericOverIp)]
#[generic_over_ip()]
#[cfg_attr(test, derivative(PartialEq(bound = "RuleInfo: PartialEq")))]
pub enum ValidationError<RuleInfo> {
    /// A rule matches on a property that is unavailable in the context in which it
    /// will be evaluated. For example, matching on the input interface in the
    /// EGRESS hook.
    RuleWithInvalidMatcher(RuleInfo),
    /// A rule has an action that is unavailable in the context in which it will be
    /// evaluated. For example, the TransparentProxy action is only valid in the
    /// INGRESS hook.
    RuleWithInvalidAction(RuleInfo),
    /// A rule has a TransparentProxy action without a corresponding valid matcher:
    /// the rule must match on transport protocol to ensure that the packet has
    /// either a TCP or UDP header.
    TransparentProxyWithInvalidMatcher(RuleInfo),
    /// A rule has a Redirect action without a corresponding valid matcher: if the
    /// action specifies a destination port range, the rule must match on transport
    /// protocol to ensure that the packet has either a TCP or UDP header.
    RedirectWithInvalidMatcher(RuleInfo),
    /// A rule has a Masquerade action without a corresponding valid matcher: if the
    /// action specifies a source port range, the rule must match on transport
    /// protocol to ensure that the packet has either a TCP or UDP header.
    MasqueradeWithInvalidMatcher(RuleInfo),
    /// A rule has a TCP-Reset Reject action without a corresponding valid
    /// matcher. RejectAction with TcpReset reject type is allowed only if the
    /// rule matches TCP.
    RejectWithInvalidMatcher(RuleInfo),
}

/// Witness type ensuring that the contained filtering state has been validated.
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct ValidRoutines<I: IpExt, BT: MatcherBindingsTypes>(Routines<I, BT, ()>);

impl<I: IpExt, BT: MatcherBindingsTypes> ValidRoutines<I, BT> {
    /// Accesses the inner state.
    pub fn get(&self) -> &Routines<I, BT, ()> {
        let Self(state) = self;
        &state
    }
}

impl<I: IpExt, BT: MatcherBindingsTypes> ValidRoutines<I, BT> {
    /// Validates the provide state and creates a new `ValidRoutines` along with a
    /// list of all uninstalled routines that are referred to from an installed
    /// routine. Returns a `ValidationError` if the state is invalid.
    ///
    /// The provided state must not contain any cyclical routine graphs (formed by
    /// rules with jump actions). The behavior in this case is unspecified but could
    /// be a deadlock or a panic, for example.
    ///
    /// # Panics
    ///
    /// Panics if the provided state includes cyclic routine graphs.
    pub fn new<RuleInfo: Clone>(
        routines: Routines<I, BT, RuleInfo>,
    ) -> Result<(Self, Vec<UninstalledRoutine<I, BT, ()>>), ValidationError<RuleInfo>> {
        let Routines { ip: ip_routines, nat: nat_routines } = &routines;

        // Ensure that no rule has a matcher that is unavailable in the context in which
        // the rule will be evaluated.
        let IpRoutines { ingress, local_ingress, egress, local_egress, forwarding } = ip_routines;
        validate_hook(
            &ingress,
            &[UnavailableMatcher::OutInterface],
            &[
                UnavailableAction::Redirect,
                UnavailableAction::Masquerade,
                UnavailableAction::Reject,
            ],
        )?;
        validate_hook(
            &local_ingress,
            &[UnavailableMatcher::OutInterface],
            &[
                UnavailableAction::TransparentProxy,
                UnavailableAction::Redirect,
                UnavailableAction::Masquerade,
            ],
        )?;
        validate_hook(
            &forwarding,
            &[],
            &[
                UnavailableAction::TransparentProxy,
                UnavailableAction::Redirect,
                UnavailableAction::Masquerade,
            ],
        )?;
        validate_hook(
            &egress,
            &[UnavailableMatcher::InInterface],
            &[
                UnavailableAction::TransparentProxy,
                UnavailableAction::Redirect,
                UnavailableAction::Masquerade,
                UnavailableAction::Reject,
            ],
        )?;
        validate_hook(
            &local_egress,
            &[UnavailableMatcher::InInterface],
            &[
                UnavailableAction::TransparentProxy,
                UnavailableAction::Redirect,
                UnavailableAction::Masquerade,
            ],
        )?;

        let NatRoutines { ingress, local_ingress, egress, local_egress } = nat_routines;
        validate_hook(
            &ingress,
            &[UnavailableMatcher::OutInterface],
            &[
                UnavailableAction::TransparentProxy,
                UnavailableAction::Masquerade,
                UnavailableAction::Mark,
                UnavailableAction::Reject,
            ],
        )?;
        validate_hook(
            &local_ingress,
            &[UnavailableMatcher::OutInterface],
            &[
                UnavailableAction::TransparentProxy,
                UnavailableAction::Redirect,
                UnavailableAction::Masquerade,
                UnavailableAction::Mark,
                UnavailableAction::Reject,
            ],
        )?;
        validate_hook(
            &egress,
            &[UnavailableMatcher::InInterface],
            &[
                UnavailableAction::TransparentProxy,
                UnavailableAction::Redirect,
                UnavailableAction::Mark,
                UnavailableAction::Reject,
            ],
        )?;
        validate_hook(
            &local_egress,
            &[UnavailableMatcher::InInterface],
            &[
                UnavailableAction::TransparentProxy,
                UnavailableAction::Masquerade,
                UnavailableAction::Mark,
                UnavailableAction::Reject,
            ],
        )?;

        let mut index = UninstalledRoutineIndex::default();
        let routines = routines.strip_debug_info(&mut index);
        Ok((Self(routines), index.into_values()))
    }
}

#[derive(Clone, Copy)]
enum UnavailableMatcher {
    InInterface,
    OutInterface,
}

impl UnavailableMatcher {
    fn validate<I: IpExt, BT: MatcherBindingsTypes, RuleInfo: Clone>(
        &self,
        matcher: &PacketMatcher<I, BT>,
        rule: &RuleInfo,
    ) -> Result<(), ValidationError<RuleInfo>> {
        let unavailable_matcher = match self {
            UnavailableMatcher::InInterface => matcher.in_interface.as_ref(),
            UnavailableMatcher::OutInterface => matcher.out_interface.as_ref(),
        };
        if unavailable_matcher.is_some() {
            Err(ValidationError::RuleWithInvalidMatcher(rule.clone()))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy)]
enum UnavailableAction {
    TransparentProxy,
    Redirect,
    Masquerade,
    Mark,
    Reject,
}

impl UnavailableAction {
    fn validate<I: IpExt, BT: MatcherBindingsTypes, RuleInfo: Clone>(
        &self,
        action: &Action<I, BT, RuleInfo>,
        rule: &RuleInfo,
    ) -> Result<(), ValidationError<RuleInfo>> {
        match (self, action) {
            (UnavailableAction::TransparentProxy, Action::TransparentProxy(_))
            | (UnavailableAction::Redirect, Action::Redirect { .. })
            | (UnavailableAction::Masquerade, Action::Masquerade { .. })
            | (UnavailableAction::Mark, Action::Mark { .. })
            | (UnavailableAction::Reject, Action::Reject(_)) => {
                Err(ValidationError::RuleWithInvalidAction(rule.clone()))
            }
            _ => Ok(()),
        }
    }
}

/// Ensures that no rules reachable from this hook match on
/// `unavailable_matcher`.
fn validate_hook<I: IpExt, BT: MatcherBindingsTypes, RuleInfo: Clone>(
    Hook { routines }: &Hook<I, BT, RuleInfo>,
    unavailable_matchers: &[UnavailableMatcher],
    unavailable_actions: &[UnavailableAction],
) -> Result<(), ValidationError<RuleInfo>> {
    for routine in routines {
        validate_routine(routine, unavailable_matchers, unavailable_actions)?;
    }

    Ok(())
}

/// Ensures that:
///  * no rules reachable from this routine match on any of the
///    `unavailable_matchers`.
///  * no rules reachable from this routine include one of the
///    `unavailable_actions`.
///  * all rules reachable from this routine have matchers that are compatible
///    with their actions (for example, specifying a port rewrite requires that
///    a transport protocol matcher be present).
fn validate_routine<I: IpExt, BT: MatcherBindingsTypes, RuleInfo: Clone>(
    Routine { rules }: &Routine<I, BT, RuleInfo>,
    unavailable_matchers: &[UnavailableMatcher],
    unavailable_actions: &[UnavailableAction],
) -> Result<(), ValidationError<RuleInfo>> {
    for Rule { matcher, action, validation_info } in rules {
        for unavailable in unavailable_matchers {
            unavailable.validate(matcher, validation_info)?;
        }
        for unavailable in unavailable_actions {
            unavailable.validate(action, validation_info)?;
        }

        let get_proto_matcher = |matcher: &PacketMatcher<_, _>| {
            let Some(TransportProtocolMatcher { proto, .. }) = matcher.transport_protocol else {
                return None;
            };
            I::map_ip_in(
                proto,
                |proto| match proto {
                    Ipv4Proto::Proto(proto) => Some(proto),
                    _ => None,
                },
                |proto| match proto {
                    Ipv6Proto::Proto(proto) => Some(proto),
                    _ => None,
                },
            )
        };

        let has_tcp_or_udp_matcher = |matcher: &PacketMatcher<_, _>| {
            matches!(get_proto_matcher(matcher), Some(IpProto::Tcp | IpProto::Udp))
        };

        match action {
            Action::Accept | Action::Drop | Action::Return | Action::Mark { .. } | Action::None => {
            }
            Action::TransparentProxy(_) => {
                // TransparentProxy is only valid in a rule that matches on
                // either TCP or UDP.
                if !has_tcp_or_udp_matcher(matcher) {
                    return Err(ValidationError::TransparentProxyWithInvalidMatcher(
                        validation_info.clone(),
                    ));
                }
            }
            Action::Redirect { dst_port } => {
                if dst_port.is_some() {
                    // Redirect can only specify a destination port in a rule
                    // that matches on either TCP or UDP.
                    if !has_tcp_or_udp_matcher(matcher) {
                        return Err(ValidationError::RedirectWithInvalidMatcher(
                            validation_info.clone(),
                        ));
                    };
                }
            }
            Action::Masquerade { src_port } => {
                if src_port.is_some() {
                    // Masquerde can only specify a source port in a rule that
                    // matches on either TCP or UDP.
                    if !has_tcp_or_udp_matcher(matcher) {
                        return Err(ValidationError::MasqueradeWithInvalidMatcher(
                            validation_info.clone(),
                        ));
                    };
                }
            }
            Action::Jump(target) => {
                let UninstalledRoutine { routine, id: _ } = target;
                validate_routine(&*routine, unavailable_matchers, unavailable_actions)?;
            }
            Action::Reject(reject_type) => match reject_type {
                RejectType::TcpReset => {
                    if get_proto_matcher(matcher) != Some(IpProto::Tcp) {
                        return Err(ValidationError::RejectWithInvalidMatcher(
                            validation_info.clone(),
                        ));
                    }
                }
                RejectType::NetUnreachable
                | RejectType::HostUnreachable
                | RejectType::ProtoUnreachable
                | RejectType::PortUnreachable
                | RejectType::RoutePolicyFail
                | RejectType::RejectRoute
                | RejectType::AdminProhibited => {}
            },
        }
    }

    Ok(())
}

#[derive(Derivative)]
#[derivative(PartialEq(bound = ""), Debug(bound = ""))]
enum ConvertedRoutine<I: IpExt, BT: MatcherBindingsTypes> {
    InProgress,
    Done(UninstalledRoutine<I, BT, ()>),
}

#[derive(Derivative)]
#[derivative(Default(bound = ""))]
struct UninstalledRoutineIndex<I: IpExt, BT: MatcherBindingsTypes, RuleInfo> {
    index: HashMap<UninstalledRoutine<I, BT, RuleInfo>, ConvertedRoutine<I, BT>>,
}

impl<I: IpExt, BT: MatcherBindingsTypes, RuleInfo: Clone> UninstalledRoutineIndex<I, BT, RuleInfo> {
    fn get_or_insert_with(
        &mut self,
        target: UninstalledRoutine<I, BT, RuleInfo>,
        convert: impl FnOnce(
            &mut UninstalledRoutineIndex<I, BT, RuleInfo>,
        ) -> UninstalledRoutine<I, BT, ()>,
    ) -> UninstalledRoutine<I, BT, ()> {
        match self.index.entry(target.clone()) {
            Entry::Occupied(entry) => match entry.get() {
                ConvertedRoutine::InProgress => panic!("cycle in routine graph"),
                ConvertedRoutine::Done(routine) => return routine.clone(),
            },
            Entry::Vacant(entry) => {
                let _ = entry.insert(ConvertedRoutine::InProgress);
            }
        }
        // Convert the target routine and store it in the index, so that the next time
        // we attempt to convert it, we just reuse the already-converted routine.
        let converted = convert(self);
        let previous = self.index.insert(target, ConvertedRoutine::Done(converted.clone()));
        assert_eq!(previous, Some(ConvertedRoutine::InProgress));
        converted
    }

    fn into_values(self) -> Vec<UninstalledRoutine<I, BT, ()>> {
        self.index
            .into_values()
            .map(|routine| assert_matches!(routine, ConvertedRoutine::Done(routine) => routine))
            .collect()
    }
}

impl<I: IpExt, BT: MatcherBindingsTypes, RuleInfo: Clone> Routines<I, BT, RuleInfo> {
    fn strip_debug_info(
        self,
        index: &mut UninstalledRoutineIndex<I, BT, RuleInfo>,
    ) -> Routines<I, BT, ()> {
        let Self { ip: ip_routines, nat: nat_routines } = self;
        Routines {
            ip: ip_routines.strip_debug_info(index),
            nat: nat_routines.strip_debug_info(index),
        }
    }
}

impl<I: IpExt, BT: MatcherBindingsTypes, RuleInfo: Clone> IpRoutines<I, BT, RuleInfo> {
    fn strip_debug_info(
        self,
        index: &mut UninstalledRoutineIndex<I, BT, RuleInfo>,
    ) -> IpRoutines<I, BT, ()> {
        let Self { ingress, local_ingress, egress, local_egress, forwarding } = self;
        IpRoutines {
            ingress: ingress.strip_debug_info(index),
            local_ingress: local_ingress.strip_debug_info(index),
            forwarding: forwarding.strip_debug_info(index),
            egress: egress.strip_debug_info(index),
            local_egress: local_egress.strip_debug_info(index),
        }
    }
}

impl<I: IpExt, BT: MatcherBindingsTypes, RuleInfo: Clone> NatRoutines<I, BT, RuleInfo> {
    fn strip_debug_info(
        self,
        index: &mut UninstalledRoutineIndex<I, BT, RuleInfo>,
    ) -> NatRoutines<I, BT, ()> {
        let Self { ingress, local_ingress, egress, local_egress } = self;
        NatRoutines {
            ingress: ingress.strip_debug_info(index),
            local_ingress: local_ingress.strip_debug_info(index),
            egress: egress.strip_debug_info(index),
            local_egress: local_egress.strip_debug_info(index),
        }
    }
}

impl<I: IpExt, BT: MatcherBindingsTypes, RuleInfo: Clone> Hook<I, BT, RuleInfo> {
    fn strip_debug_info(
        self,
        index: &mut UninstalledRoutineIndex<I, BT, RuleInfo>,
    ) -> Hook<I, BT, ()> {
        let Self { routines } = self;
        Hook {
            routines: routines.into_iter().map(|routine| routine.strip_debug_info(index)).collect(),
        }
    }
}

impl<I: IpExt, BT: MatcherBindingsTypes, RuleInfo: Clone> Routine<I, BT, RuleInfo> {
    fn strip_debug_info(
        self,
        index: &mut UninstalledRoutineIndex<I, BT, RuleInfo>,
    ) -> Routine<I, BT, ()> {
        let Self { rules } = self;
        Routine {
            rules: rules
                .into_iter()
                .map(|Rule { matcher, action, validation_info: _ }| Rule {
                    matcher,
                    action: action.strip_debug_info(index),
                    validation_info: (),
                })
                .collect(),
        }
    }
}

impl<I: IpExt, BT: MatcherBindingsTypes, RuleInfo: Clone> Action<I, BT, RuleInfo> {
    fn strip_debug_info(
        self,
        index: &mut UninstalledRoutineIndex<I, BT, RuleInfo>,
    ) -> Action<I, BT, ()> {
        match self {
            Self::Accept => Action::Accept,
            Self::Drop => Action::Drop,
            Self::Return => Action::Return,
            Self::TransparentProxy(proxy) => Action::TransparentProxy(proxy),
            Self::Redirect { dst_port } => Action::Redirect { dst_port },
            Self::Masquerade { src_port } => Action::Masquerade { src_port },
            Self::Mark { domain, action } => Action::Mark { domain, action },
            Self::Jump(target) => {
                let converted = index.get_or_insert_with(target.clone(), |index| {
                    // Recursively strip debug info from the target routine.
                    let UninstalledRoutine { ref routine, id } = target;
                    UninstalledRoutine {
                        routine: Arc::new(Routine::clone(&*routine).strip_debug_info(index)),
                        id,
                    }
                });
                Action::Jump(converted)
            }
            Self::None => Action::None,
            Self::Reject(reject_type) => Action::Reject(reject_type),
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use core::num::NonZeroU16;

    use assert_matches::assert_matches;
    use ip_test_macro::ip_test;
    use net_types::ip::Ipv4;
    use netstack3_base::InterfaceMatcher;
    use netstack3_base::testutil::FakeDeviceClass;
    use test_case::test_case;

    use super::*;
    use crate::context::testutil::FakeBindingsCtx;
    use crate::{PacketMatcher, TransparentProxy};

    #[derive(Debug, Clone, PartialEq)]
    enum RuleId {
        Valid,
        Invalid,
    }

    fn rule<I: IpExt>(
        matcher: PacketMatcher<I, FakeBindingsCtx<I>>,
        validation_info: RuleId,
    ) -> Rule<I, FakeBindingsCtx<I>, RuleId> {
        Rule { matcher, action: Action::Drop, validation_info }
    }

    fn hook_with_rules<I: IpExt>(
        rules: Vec<Rule<I, FakeBindingsCtx<I>, RuleId>>,
    ) -> Hook<I, FakeBindingsCtx<I>, RuleId> {
        Hook { routines: vec![Routine { rules }] }
    }

    #[ip_test(I)]
    #[test_case(
        hook_with_rules(vec![rule(
            PacketMatcher {
                in_interface: Some(InterfaceMatcher::DeviceClass(FakeDeviceClass::Ethernet)),
                ..Default::default()
            },
            RuleId::Valid,
        )]),
        UnavailableMatcher::OutInterface =>
        Ok(());
        "match on input interface in root routine when available"
    )]
    #[test_case(
        hook_with_rules(vec![rule(
            PacketMatcher {
                out_interface: Some(InterfaceMatcher::DeviceClass(FakeDeviceClass::Ethernet)),
                ..Default::default()
            },
            RuleId::Valid,
        )]),
        UnavailableMatcher::InInterface =>
        Ok(());
        "match on output interface in root routine when available"
    )]
    #[test_case(
        hook_with_rules(vec![
            rule(PacketMatcher::default(), RuleId::Valid),
            rule(
                PacketMatcher {
                    in_interface: Some(InterfaceMatcher::DeviceClass(FakeDeviceClass::Ethernet)),
                    ..Default::default()
                },
                RuleId::Invalid,
            ),
        ]),
        UnavailableMatcher::InInterface =>
        Err(ValidationError::RuleWithInvalidMatcher(RuleId::Invalid));
        "match on input interface in root routine when unavailable"
    )]
    #[test_case(
        hook_with_rules(vec![
            rule(PacketMatcher::default(), RuleId::Valid),
            rule(
                PacketMatcher {
                    out_interface: Some(InterfaceMatcher::DeviceClass(FakeDeviceClass::Ethernet)),
                    ..Default::default()
                },
                RuleId::Invalid,
            ),
        ]),
        UnavailableMatcher::OutInterface =>
        Err(ValidationError::RuleWithInvalidMatcher(RuleId::Invalid));
        "match on output interface in root routine when unavailable"
    )]
    #[test_case(
        Hook {
            routines: vec![Routine {
                rules: vec![Rule {
                    matcher: PacketMatcher::default(),
                    action: Action::Jump(UninstalledRoutine::new(
                        vec![rule(
                            PacketMatcher {
                                in_interface: Some(InterfaceMatcher::DeviceClass(
                                    FakeDeviceClass::Ethernet,
                                )),
                                ..Default::default()
                            },
                            RuleId::Invalid,
                        )],
                        0,
                    )),
                    validation_info: RuleId::Valid,
                }],
            }],
        },
        UnavailableMatcher::InInterface =>
        Err(ValidationError::RuleWithInvalidMatcher(RuleId::Invalid));
        "match on input interface in target routine when unavailable"
    )]
    #[test_case(
        Hook {
            routines: vec![Routine {
                rules: vec![Rule {
                    matcher: PacketMatcher::default(),
                    action: Action::Jump(UninstalledRoutine::new(
                        vec![rule(
                            PacketMatcher {
                                out_interface: Some(InterfaceMatcher::DeviceClass(
                                    FakeDeviceClass::Ethernet,
                                )),
                                ..Default::default()
                            },
                            RuleId::Invalid,
                        )],
                        0,
                    )),
                    validation_info: RuleId::Valid,
                }],
            }],
        },
        UnavailableMatcher::OutInterface =>
        Err(ValidationError::RuleWithInvalidMatcher(RuleId::Invalid));
        "match on output interface in target routine when unavailable"
    )]
    fn validate_interface_matcher_available<I: IpExt>(
        hook: Hook<I, FakeBindingsCtx<I>, RuleId>,
        unavailable_matcher: UnavailableMatcher,
    ) -> Result<(), ValidationError<RuleId>> {
        validate_hook(&hook, &[unavailable_matcher], &[])
    }

    fn hook_with_rule<I: IpExt>(
        rule: Rule<I, FakeBindingsCtx<I>, RuleId>,
    ) -> Hook<I, FakeBindingsCtx<I>, RuleId> {
        Hook { routines: vec![Routine { rules: vec![rule] }] }
    }

    fn transport_matcher<I: IpExt>(proto: I::Proto) -> PacketMatcher<I, FakeBindingsCtx<I>> {
        PacketMatcher {
            transport_protocol: Some(TransportProtocolMatcher {
                proto,
                src_port: None,
                dst_port: None,
            }),
            ..Default::default()
        }
    }

    fn udp_matcher<I: IpExt>() -> PacketMatcher<I, FakeBindingsCtx<I>> {
        transport_matcher(I::map_ip(
            (),
            |()| Ipv4Proto::Proto(IpProto::Udp),
            |()| Ipv6Proto::Proto(IpProto::Udp),
        ))
    }

    fn tcp_matcher<I: IpExt>() -> PacketMatcher<I, FakeBindingsCtx<I>> {
        transport_matcher(I::map_ip(
            (),
            |()| Ipv4Proto::Proto(IpProto::Tcp),
            |()| Ipv6Proto::Proto(IpProto::Tcp),
        ))
    }

    fn icmp_matcher<I: IpExt>() -> PacketMatcher<I, FakeBindingsCtx<I>> {
        transport_matcher(I::map_ip((), |()| Ipv4Proto::Icmp, |()| Ipv6Proto::Icmpv6))
    }

    const LOCAL_PORT: NonZeroU16 = NonZeroU16::new(8080).unwrap();

    #[ip_test(I)]
    #[test_case(
        Routines {
            ip: IpRoutines {
                ingress: hook_with_rule(Rule {
                    matcher: udp_matcher(),
                    action: Action::TransparentProxy(TransparentProxy::LocalPort(LOCAL_PORT)),
                    validation_info: RuleId::Valid,
                }),
                ..Default::default()
            },
            ..Default::default()
        } =>
        Ok(());
        "transparent proxy available in IP INGRESS routines"
    )]
    #[test_case(
        Routines {
            ip: IpRoutines {
                ingress: hook_with_rule(Rule {
                    matcher: PacketMatcher::default(),
                    action: Action::Jump(UninstalledRoutine::new(
                        vec![Rule {
                            matcher: udp_matcher(),
                            action: Action::TransparentProxy(
                                TransparentProxy::LocalPort(LOCAL_PORT)
                            ),
                            validation_info: RuleId::Valid,
                        }],
                        0,
                    )),
                    validation_info: RuleId::Valid,
                }),
                ..Default::default()
            },
            ..Default::default()
        } =>
        Ok(());
        "transparent proxy available in target routine reachable from INGRESS"
    )]
    #[test_case(
        Routines {
            ip: IpRoutines {
                egress: hook_with_rule(Rule {
                    matcher: udp_matcher(),
                    action: Action::TransparentProxy(TransparentProxy::LocalPort(LOCAL_PORT)),
                    validation_info: RuleId::Invalid,
                }),
                ..Default::default()
            },
            ..Default::default()
        } =>
        Err(ValidationError::RuleWithInvalidAction(RuleId::Invalid));
        "transparent proxy unavailable in IP EGRESS routine"
    )]
    #[test_case(
        Routines {
            ip: IpRoutines {
                egress: hook_with_rule(Rule {
                    matcher: PacketMatcher::default(),
                    action: Action::Jump(UninstalledRoutine::new(
                        vec![Rule {
                            matcher: udp_matcher(),
                            action: Action::TransparentProxy(
                                TransparentProxy::LocalPort(LOCAL_PORT)
                            ),
                            validation_info: RuleId::Invalid,
                        }],
                        0,
                    )),
                    validation_info: RuleId::Valid,
                }),
                ..Default::default()
            },
            ..Default::default()
        } =>
        Err(ValidationError::RuleWithInvalidAction(RuleId::Invalid));
        "transparent proxy unavailable in target routine reachable from EGRESS"
    )]
    #[test_case(
        Routines {
            nat: NatRoutines {
                ingress: hook_with_rule(Rule {
                    matcher: PacketMatcher::default(),
                    action: Action::Redirect { dst_port: None },
                    validation_info: RuleId::Valid,
                }),
                local_egress: hook_with_rule(Rule {
                    matcher: PacketMatcher::default(),
                    action: Action::Redirect { dst_port: None },
                    validation_info: RuleId::Valid,
                }),
                ..Default::default()
            },
            ..Default::default()
        } =>
        Ok(());
        "redirect available in NAT INGRESS and LOCAL_EGRESS routines"
    )]
    #[test_case(
        Routines {
            nat: NatRoutines {
                egress: hook_with_rule(Rule {
                    matcher: PacketMatcher::default(),
                    action: Action::Redirect { dst_port: None },
                    validation_info: RuleId::Invalid,
                }),
                ..Default::default()
            },
            ..Default::default()
        } =>
        Err(ValidationError::RuleWithInvalidAction(RuleId::Invalid));
        "redirect unavailable in NAT EGRESS"
    )]
    #[test_case(
        Routines {
            ip: IpRoutines {
                ingress: hook_with_rule(Rule {
                    matcher: PacketMatcher::default(),
                    action: Action::Redirect { dst_port: None },
                    validation_info: RuleId::Invalid,
                }),
                ..Default::default()
            },
            ..Default::default()
        } =>
        Err(ValidationError::RuleWithInvalidAction(RuleId::Invalid));
        "redirect unavailable in IP routines"
    )]
    #[test_case(
        Routines {
            nat: NatRoutines {
                egress: hook_with_rule(Rule {
                    matcher: PacketMatcher::default(),
                    action: Action::Masquerade { src_port: None },
                    validation_info: RuleId::Valid,
                }),
                ..Default::default()
            },
            ..Default::default()
        } =>
        Ok(());
        "masquerade available in NAT EGRESS"
    )]
    #[test_case(
        Routines {
            nat: NatRoutines {
                local_ingress: hook_with_rule(Rule {
                    matcher: PacketMatcher::default(),
                    action: Action::Masquerade { src_port: None },
                    validation_info: RuleId::Invalid,
                }),
                ..Default::default()
            },
            ..Default::default()
        } =>
        Err(ValidationError::RuleWithInvalidAction(RuleId::Invalid));
        "masquerade unavailable in NAT LOCAL_INGRESS"
    )]
    #[test_case(
        Routines {
            ip: IpRoutines {
                egress: hook_with_rule(Rule {
                    matcher: PacketMatcher::default(),
                    action: Action::Masquerade { src_port: None },
                    validation_info: RuleId::Invalid,
                }),
                ..Default::default()
            },
            ..Default::default()
        } =>
        Err(ValidationError::RuleWithInvalidAction(RuleId::Invalid));
        "masquerade unavailable in IP routines"
    )]
    fn validate_action_available<I: IpExt>(
        routines: Routines<I, FakeBindingsCtx<I>, RuleId>,
    ) -> Result<(), ValidationError<RuleId>> {
        ValidRoutines::new(routines).map(|_| ())
    }

    #[ip_test(I)]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: tcp_matcher(),
                action: Action::TransparentProxy(TransparentProxy::LocalPort(LOCAL_PORT)),
                validation_info: RuleId::Valid,
            }],
        } =>
        Ok(());
        "transparent proxy valid with TCP matcher"
    )]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: udp_matcher(),
                action: Action::TransparentProxy(TransparentProxy::LocalPort(LOCAL_PORT)),
                validation_info: RuleId::Valid,
            }],
        } =>
        Ok(());
        "transparent proxy valid with UDP matcher"
    )]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: icmp_matcher(),
                action: Action::TransparentProxy(TransparentProxy::LocalPort(LOCAL_PORT)),
                validation_info: RuleId::Invalid,
            }],
        } =>
        Err(ValidationError::TransparentProxyWithInvalidMatcher(RuleId::Invalid));
        "transparent proxy invalid with ICMP matcher"
    )]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: PacketMatcher::default(),
                action: Action::TransparentProxy(TransparentProxy::LocalPort(LOCAL_PORT)),
                validation_info: RuleId::Invalid,
            }],
        } =>
        Err(ValidationError::TransparentProxyWithInvalidMatcher(RuleId::Invalid));
        "transparent proxy invalid with no transport protocol matcher"
    )]
    fn validate_transparent_proxy_matcher<I: IpExt>(
        routine: Routine<I, FakeBindingsCtx<I>, RuleId>,
    ) -> Result<(), ValidationError<RuleId>> {
        validate_routine(&routine, &[], &[])
    }

    #[ip_test(I)]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: PacketMatcher::default(),
                action: Action::Redirect { dst_port: None },
                validation_info: RuleId::Valid,
            }],
        } =>
        Ok(());
        "redirect valid with no matcher if dst port unspecified"
    )]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: tcp_matcher(),
                action: Action::Redirect { dst_port: Some(LOCAL_PORT..=LOCAL_PORT) },
                validation_info: RuleId::Valid,
            }],
        } =>
        Ok(());
        "redirect valid with TCP matcher when dst port specified"
    )]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: udp_matcher(),
                action: Action::Redirect { dst_port: Some(LOCAL_PORT..=LOCAL_PORT) },
                validation_info: RuleId::Valid,
            }],
        } =>
        Ok(());
        "redirect valid with UDP matcher when dst port specified"
    )]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: icmp_matcher(),
                action: Action::Redirect { dst_port: Some(LOCAL_PORT..=LOCAL_PORT) },
                validation_info: RuleId::Invalid,
            }],
        } =>
        Err(ValidationError::RedirectWithInvalidMatcher(RuleId::Invalid));
        "redirect invalid with ICMP matcher when dst port specified"
    )]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: PacketMatcher::default(),
                action: Action::Redirect { dst_port: Some(LOCAL_PORT..=LOCAL_PORT) },
                validation_info: RuleId::Invalid,
            }],
        } =>
        Err(ValidationError::RedirectWithInvalidMatcher(RuleId::Invalid));
        "redirect invalid with no transport protocol matcher when dst port specified"
    )]
    fn validate_redirect_matcher<I: IpExt>(
        routine: Routine<I, FakeBindingsCtx<I>, RuleId>,
    ) -> Result<(), ValidationError<RuleId>> {
        validate_routine(&routine, &[], &[])
    }

    #[ip_test(I)]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: PacketMatcher::default(),
                action: Action::Masquerade { src_port: None },
                validation_info: RuleId::Valid,
            }],
        } =>
        Ok(());
        "masquerade valid with no matcher if src port unspecified"
    )]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: tcp_matcher(),
                action: Action::Masquerade { src_port: Some(LOCAL_PORT..=LOCAL_PORT) },
                validation_info: RuleId::Valid,
            }],
        } =>
        Ok(());
        "masquerade valid with TCP matcher when src port specified"
    )]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: udp_matcher(),
                action: Action::Masquerade { src_port: Some(LOCAL_PORT..=LOCAL_PORT) },
                validation_info: RuleId::Valid,
            }],
        } =>
        Ok(());
        "masquerade valid with UDP matcher when src port specified"
    )]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: icmp_matcher(),
                action: Action::Masquerade { src_port: Some(LOCAL_PORT..=LOCAL_PORT) },
                validation_info: RuleId::Invalid,
            }],
        } =>
        Err(ValidationError::MasqueradeWithInvalidMatcher(RuleId::Invalid));
        "masquerade invalid with ICMP matcher when src port specified"
    )]
    #[test_case(
        Routine {
            rules: vec![Rule {
                matcher: PacketMatcher::default(),
                action: Action::Masquerade { src_port: Some(LOCAL_PORT..=LOCAL_PORT) },
                validation_info: RuleId::Invalid,
            }],
        } =>
        Err(ValidationError::MasqueradeWithInvalidMatcher(RuleId::Invalid));
        "masquerade invalid with no transport protocol matcher when src port specified"
    )]
    fn validate_masquerade_matcher<I: IpExt>(
        routine: Routine<I, FakeBindingsCtx<I>, RuleId>,
    ) -> Result<(), ValidationError<RuleId>> {
        validate_routine(&routine, &[], &[])
    }

    #[test]
    fn strip_debug_info_reuses_uninstalled_routines() {
        // Two routines in the hook jump to the same uninstalled routine.
        let uninstalled_routine =
            UninstalledRoutine::<Ipv4, FakeBindingsCtx<Ipv4>, _>::new(Vec::new(), 0);
        let hook = Hook {
            routines: vec![
                Routine {
                    rules: vec![Rule {
                        matcher: PacketMatcher::default(),
                        action: Action::Jump(uninstalled_routine.clone()),
                        validation_info: "rule-1",
                    }],
                },
                Routine {
                    rules: vec![Rule {
                        matcher: PacketMatcher::default(),
                        action: Action::Jump(uninstalled_routine),
                        validation_info: "rule-2",
                    }],
                },
            ],
        };

        // When we strip the debug info from the routines in the hook, all
        // jump targets should be converted 1:1. In this case, there are two
        // jump actions that refer to the same uninstalled routine, so that
        // uninstalled routine should be converted once, and the resulting jump
        // actions should both point to the same new uninstalled routine.
        let Hook { routines } = hook.strip_debug_info(&mut UninstalledRoutineIndex::default());
        let (first, second) = assert_matches!(
            &routines[..],
            [Routine { rules: first }, Routine { rules: second }] => (first, second)
        );
        let first = assert_matches!(
            &first[..],
            [Rule { action: Action::Jump(target), .. }] => target
        );
        let second = assert_matches!(
            &second[..],
            [Rule { action: Action::Jump(target), .. }] => target
        );
        assert_eq!(first, second);
    }
}
