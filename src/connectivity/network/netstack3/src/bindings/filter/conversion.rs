// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod actions;
mod matchers;

use std::collections::{BTreeMap, HashMap};

use assert_matches::assert_matches;
use fidl_fuchsia_net_filter_ext as fnet_filter_ext;
use net_types::ip::{GenericOverIp, Ip, Ipv4, Ipv6};
use packet_formats::ip::IpExt;

use crate::bindings::ctx::BindingsCtx;
use crate::bindings::filter::CommitError;
use crate::bindings::filter::controller::{
    InstalledIpRoutine, InstalledNatRoutine, IpRoutineType, Namespace, NatRoutineType, Routine,
    Rule,
};
use crate::bindings::util::{IntoCore as _, TryFromFidl};

// The requirement or lack thereof that a particular resource's IP version (for
// example, whether an address matcher is IPv4 or IPv6) must match the version
// of the state that it is being added to.
//
// For a namespace with a specific IP domain, this is `IpVersionMustMatchState`;
// for a namespace with an IP-agnostic domain, this is
// `IpVersionCanDifferFromState`.
#[derive(Clone, Copy)]
enum IpVersionStrictness {
    IpVersionMustMatchState,
    IpVersionCanDifferFromState,
}

impl IpVersionStrictness {
    pub(super) fn mismatch_result<T>(&self) -> Result<ConversionResult<T>, ConversionError> {
        match self {
            IpVersionStrictness::IpVersionMustMatchState => Err(ConversionError::IpVersionMismatch),
            IpVersionStrictness::IpVersionCanDifferFromState => Ok(ConversionResult::Omit),
        }
    }
}

enum ConversionResult<CoreState> {
    State(CoreState),
    Omit,
}

pub(super) enum ConversionError {
    IpVersionMismatch,
}

trait TryConvertToCoreState {
    type CoreState<I: IpExt>;

    /// Attempts to convert `self` to the associated `netstack3_core::filter::state` type.
    ///
    /// If the IP version with which the method is invoked does not match
    /// `self` (for example, if `self` contains an IPv4-specific address matcher
    /// and this method is called with `Ipv6`), returns either:
    ///
    ///   * Err(ConversionError::IpVersionMismatch) if `ip_version_strictness` is
    ///     `IpVersionMustMatchState`
    ///   * Ok(ConversionResult::Omit) otherwise
    fn try_convert<I: IpExt>(
        self,
        ip_version_strictness: IpVersionStrictness,
    ) -> Result<ConversionResult<Self::CoreState<I>>, ConversionError>;
}

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Ord, Eq)]
struct RoutinePriority {
    priority: i32,
    installation_order: usize,
}

type CoreRule<I> = netstack3_core::filter::Rule<I, BindingsCtx, fnet_filter_ext::RuleId>;
type CoreRoutine<I> = netstack3_core::filter::Routine<I, BindingsCtx, fnet_filter_ext::RuleId>;
type CoreUninstalledRoutine<I> =
    netstack3_core::filter::UninstalledRoutine<I, BindingsCtx, fnet_filter_ext::RuleId>;
type CoreHook<I> = netstack3_core::filter::Hook<I, BindingsCtx, fnet_filter_ext::RuleId>;
type CoreRoutines<I> = netstack3_core::filter::Routines<I, BindingsCtx, fnet_filter_ext::RuleId>;

#[derive(Clone, Debug, Default, GenericOverIp)]
#[generic_over_ip(I, Ip)]
struct IpRoutines<I: IpExt> {
    ingress: BTreeMap<RoutinePriority, CoreRoutine<I>>,
    local_ingress: BTreeMap<RoutinePriority, CoreRoutine<I>>,
    forwarding: BTreeMap<RoutinePriority, CoreRoutine<I>>,
    local_egress: BTreeMap<RoutinePriority, CoreRoutine<I>>,
    egress: BTreeMap<RoutinePriority, CoreRoutine<I>>,
}

#[derive(Clone, Debug, Default, GenericOverIp)]
#[generic_over_ip(I, Ip)]
struct NatRoutines<I: IpExt> {
    ingress: BTreeMap<RoutinePriority, CoreRoutine<I>>,
    local_ingress: BTreeMap<RoutinePriority, CoreRoutine<I>>,
    local_egress: BTreeMap<RoutinePriority, CoreRoutine<I>>,
    egress: BTreeMap<RoutinePriority, CoreRoutine<I>>,
}

/// This is state that has almost entirely been converted to
/// [`netstack3_core::filter::Routines`], but which retains some auxiliary
/// information to make it easy to merge with other controllers' state. In
/// particular, it stores routines as BTreeMaps keyed by the routine's priority,
/// which allows routines from other controllers to be merged into a single set
/// of IP and NAT hooks.
#[derive(Clone, Debug, Default, GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub(super) struct State<I: IpExt> {
    ip_routines: IpRoutines<I>,
    nat_routines: NatRoutines<I>,
}

impl<I: IpExt> State<I> {
    pub fn merge(&mut self, other: &Self) {
        fn merge_hook<I: IpExt>(
            dst: &mut BTreeMap<RoutinePriority, CoreRoutine<I>>,
            src: &BTreeMap<RoutinePriority, CoreRoutine<I>>,
        ) {
            for (priority, routine) in src {
                assert_matches!(dst.insert(priority.clone(), routine.clone()), None);
            }
        }

        let State { ip_routines, nat_routines } = self;

        merge_hook(&mut ip_routines.ingress, &other.ip_routines.ingress);
        merge_hook(&mut ip_routines.local_ingress, &other.ip_routines.local_ingress);
        merge_hook(&mut ip_routines.forwarding, &other.ip_routines.forwarding);
        merge_hook(&mut ip_routines.local_egress, &other.ip_routines.local_egress);
        merge_hook(&mut ip_routines.egress, &other.ip_routines.egress);

        merge_hook(&mut nat_routines.ingress, &other.nat_routines.ingress);
        merge_hook(&mut nat_routines.local_ingress, &other.nat_routines.local_ingress);
        merge_hook(&mut nat_routines.local_egress, &other.nat_routines.local_egress);
        merge_hook(&mut nat_routines.egress, &other.nat_routines.egress);
    }

    fn add_namespace(
        &mut self,
        namespace: &fnet_filter_ext::NamespaceId,
        installed_routines: Vec<InstalledRoutine>,
        uninstalled_routines: HashMap<String, UninstalledRoutine>,
        ip_version_strictness: IpVersionStrictness,
    ) -> Result<(), CommitError> {
        // We recursively convert all uninstalled routines to their `netstack3_core`
        // equivalents up front, so that later, when converting *installed* routines, we
        // can expect all jump targets (which are always uninstalled routines) to have
        // already been "resolved", or fully converted to their `netstack3_core`
        // representation.
        let uninstalled_routines = CoreUninstalledRoutines::convert_routines(
            uninstalled_routines,
            namespace,
            ip_version_strictness,
        )?;

        for InstalledRoutine { name, routine_type, rules } in installed_routines {
            let (hook, routine_priority, routine_type) = match routine_type {
                InstalledRoutineType::Ip(InstalledIpRoutine {
                    hook,
                    priority,
                    installation_order,
                }) => {
                    let hook = match hook {
                        fnet_filter_ext::IpHook::Ingress => &mut self.ip_routines.ingress,
                        fnet_filter_ext::IpHook::LocalIngress => {
                            &mut self.ip_routines.local_ingress
                        }
                        fnet_filter_ext::IpHook::Forwarding => &mut self.ip_routines.forwarding,
                        fnet_filter_ext::IpHook::LocalEgress => &mut self.ip_routines.local_egress,
                        fnet_filter_ext::IpHook::Egress => &mut self.ip_routines.egress,
                    };
                    (hook, RoutinePriority { priority, installation_order }, RoutineType::Ip)
                }
                InstalledRoutineType::Nat(InstalledNatRoutine {
                    hook,
                    priority,
                    installation_order,
                }) => {
                    let hook = match hook {
                        fnet_filter_ext::NatHook::Ingress => &mut self.nat_routines.ingress,
                        fnet_filter_ext::NatHook::LocalIngress => {
                            &mut self.nat_routines.local_ingress
                        }
                        fnet_filter_ext::NatHook::LocalEgress => {
                            &mut self.nat_routines.local_egress
                        }
                        fnet_filter_ext::NatHook::Egress => &mut self.nat_routines.egress,
                    };
                    (hook, RoutinePriority { priority, installation_order }, RoutineType::Nat)
                }
            };

            let rules = convert_rules(
                namespace,
                &name,
                rules,
                ip_version_strictness,
                /* resolve_jump_target */
                |name, rule_id| {
                    uninstalled_routines.expect_routine_resolved(&name, routine_type, rule_id)
                },
            )?;

            assert_matches!(hook.insert(routine_priority, CoreRoutine { rules }), None);
        }
        Ok(())
    }
}

impl<I: IpExt> From<State<I>> for CoreRoutines<I> {
    fn from(state: State<I>) -> Self {
        fn core_hook<I: IpExt>(routines: BTreeMap<RoutinePriority, CoreRoutine<I>>) -> CoreHook<I> {
            CoreHook { routines: routines.into_values().collect() }
        }

        let State { ip_routines, nat_routines } = state;
        CoreRoutines {
            ip: netstack3_core::filter::IpRoutines {
                ingress: core_hook(ip_routines.ingress),
                local_ingress: core_hook(ip_routines.local_ingress),
                forwarding: core_hook(ip_routines.forwarding),
                local_egress: core_hook(ip_routines.local_egress),
                egress: core_hook(ip_routines.egress),
            },
            nat: netstack3_core::filter::NatRoutines {
                ingress: core_hook(nat_routines.ingress),
                local_ingress: core_hook(nat_routines.local_ingress),
                local_egress: core_hook(nat_routines.local_egress),
                egress: core_hook(nat_routines.egress),
            },
        }
    }
}

impl TryFromFidl<fnet_filter_ext::MarkAction> for netstack3_core::filter::MarkAction {
    type Error = std::convert::Infallible;

    fn try_from_fidl(fidl: fnet_filter_ext::MarkAction) -> Result<Self, Self::Error> {
        match fidl {
            fnet_filter_ext::MarkAction::SetMark { clearing_mask, mark } => {
                Ok(Self::SetMark { clearing_mask, mark })
            }
        }
    }
}

impl TryFromFidl<fnet_filter_ext::RejectType> for netstack3_core::filter::RejectType {
    type Error = std::convert::Infallible;

    fn try_from_fidl(fidl: fnet_filter_ext::RejectType) -> Result<Self, Self::Error> {
        match fidl {
            fnet_filter_ext::RejectType::TcpReset => Ok(Self::TcpReset),
            fnet_filter_ext::RejectType::NetUnreachable => Ok(Self::NetUnreachable),
            fnet_filter_ext::RejectType::HostUnreachable => Ok(Self::HostUnreachable),
            fnet_filter_ext::RejectType::ProtoUnreachable => Ok(Self::ProtoUnreachable),
            fnet_filter_ext::RejectType::PortUnreachable => Ok(Self::PortUnreachable),
            fnet_filter_ext::RejectType::RoutePolicyFail => Ok(Self::RoutePolicyFail),
            fnet_filter_ext::RejectType::RejectRoute => Ok(Self::RejectRoute),
            fnet_filter_ext::RejectType::AdminProhibited => Ok(Self::AdminProhibited),
        }
    }
}

fn convert_rules<I, F>(
    namespace: &fnet_filter_ext::NamespaceId,
    routine: &str,
    rules: BTreeMap<u32, Rule>,
    ip_version_strictness: IpVersionStrictness,
    mut resolve_jump_target: F,
) -> Result<Vec<CoreRule<I>>, CommitError>
where
    I: IpExt,
    F: FnMut(String, fnet_filter_ext::RuleId) -> Result<CoreUninstalledRoutine<I>, CommitError>,
{
    rules
        .into_iter()
        .filter_map(|(index, Rule { matchers, action })| {
            let rule_id = fnet_filter_ext::RuleId {
                routine: fnet_filter_ext::RoutineId {
                    namespace: namespace.to_owned(),
                    name: routine.to_owned(),
                },
                index,
            };

            let matcher = match matchers.try_convert(ip_version_strictness) {
                Ok(ConversionResult::State(matcher)) => matcher,
                Ok(ConversionResult::Omit) => return None,
                Err(_) => {
                    return Some(Err(CommitError::RuleWithInvalidMatcher(rule_id)));
                }
            };

            let action = match action {
                fnet_filter_ext::Action::Accept => netstack3_core::filter::Action::Accept,
                fnet_filter_ext::Action::Drop => netstack3_core::filter::Action::Drop,
                fnet_filter_ext::Action::Return => netstack3_core::filter::Action::Return,
                fnet_filter_ext::Action::Jump(name) => {
                    let target = match resolve_jump_target(name, rule_id.clone()) {
                        Ok(target) => target,
                        Err(e) => return Some(Err(e)),
                    };
                    netstack3_core::filter::Action::Jump(target)
                }
                fnet_filter_ext::Action::TransparentProxy(proxy) => {
                    let proxy = match proxy.try_convert(ip_version_strictness) {
                        Ok(ConversionResult::State(proxy)) => proxy,
                        Ok(ConversionResult::Omit) => return None,
                        Err(_) => {
                            return Some(Err(CommitError::RuleWithInvalidAction(rule_id)));
                        }
                    };
                    netstack3_core::filter::Action::TransparentProxy(proxy)
                }
                fnet_filter_ext::Action::Redirect { dst_port } => {
                    netstack3_core::filter::Action::Redirect {
                        dst_port: dst_port.map(|fnet_filter_ext::PortRange(range)| range),
                    }
                }
                fnet_filter_ext::Action::Masquerade { src_port } => {
                    netstack3_core::filter::Action::Masquerade {
                        src_port: src_port.map(|fnet_filter_ext::PortRange(range)| range),
                    }
                }
                fnet_filter_ext::Action::Mark { domain, action } => {
                    netstack3_core::filter::Action::Mark {
                        domain: domain.into_core(),
                        action: action.into_core(),
                    }
                }
                fnet_filter_ext::Action::None => netstack3_core::filter::Action::None,
                fnet_filter_ext::Action::Reject(reject_type) => {
                    netstack3_core::filter::Action::Reject(reject_type.into_core())
                }
            };

            Some(Ok(CoreRule { matcher, action, validation_info: rule_id }))
        })
        .collect::<Result<Vec<_>, _>>()
}

/// This is a collection of uninstalled routines that have been converted to the
/// equivalent [`netstack3_core::filter::UninstalledRoutine`] type, but which
/// organizes them by type (IP vs. NAT) and keys them by routine name, so that
/// as further installed routines are converted to Core state, jump actions can
/// reuse these routines that have already been converted.
///
/// Note that because [`netstack3_core::filter::UninstalledRoutine`] is a
/// reference-counted pointer to the underlying routine, this type can safely be
/// dropped once all other state has been converted; actions that jump to other
/// routines will hold a reference to their jump target, which will keep that
/// [`netstack3_core::filter::UninstalledRoutine`] alive.
#[derive(Default)]
struct CoreUninstalledRoutines<I: IpExt> {
    ip: HashMap<String, CoreUninstalledRoutine<I>>,
    nat: HashMap<String, CoreUninstalledRoutine<I>>,
    routine_types: HashMap<String, RoutineType>,
}

impl<I: IpExt> CoreUninstalledRoutines<I> {
    /// Converts all uninstalled routines to their `netstack3_core` equivalents up
    /// front, and checks for cycles in each routine graph by keeping track of which
    /// routines we've seen thus far, and returning an error if we ever encounter
    /// the same routine twice.
    fn convert_routines(
        mut uninstalled_routines: HashMap<String, UninstalledRoutine>,
        namespace: &fnet_filter_ext::NamespaceId,
        ip_version_strictness: IpVersionStrictness,
    ) -> Result<Self, CommitError> {
        // As uninstalled routines are converted, they are removed from
        // `uninstalled_routines` and added to `core_uninstalled`.
        let mut core_uninstalled = CoreUninstalledRoutines::default();
        while !uninstalled_routines.is_empty() {
            let name = uninstalled_routines.keys().next().expect("should not be empty").clone();

            let _ = core_uninstalled.convert_routine(
                &namespace,
                &name,
                &mut uninstalled_routines,
                ip_version_strictness,
            )?;
        }
        Ok(core_uninstalled)
    }

    /// Converts the uninstalled routine identified by `name` to its
    /// `netstack3_core` equivalent, which involves also recursively converting any
    /// routines referenced in `Jump` actions in the routine. The converted routine
    /// is stored in either `self.nat` or `self.ip` depending on its type, and its
    /// type is stored in `self.routine_types`.
    fn convert_routine(
        &mut self,
        namespace: &fnet_filter_ext::NamespaceId,
        name: &str,
        uninstalled_routines: &mut HashMap<String, UninstalledRoutine>,
        ip_version_strictness: IpVersionStrictness,
    ) -> Result<CoreUninstalledRoutine<I>, CommitError> {
        // Because we remove a routine from `uninstalled_routines` whenever we begin
        // converting it to its Core representation, failing to remove a routine
        // here means that we have encountered it twice in the same callstack, which
        // can only happen if there is a cycle in the routine graph formed by `Jump`
        // actions.
        //
        // (NB: by this point, `Jump` targets have already been validated to refer
        // to existing uninstalled routines.)
        let UninstalledRoutine { routine_type, rules, id } =
            uninstalled_routines.remove(name).ok_or_else(|| {
                CommitError::CyclicalRoutineGraph(fnet_filter_ext::RoutineId {
                    namespace: namespace.to_owned(),
                    name: name.to_owned(),
                })
            })?;

        let rules = convert_rules(
            &namespace,
            name,
            rules,
            ip_version_strictness,
            // To resolve the target routine of a `Jump` action, either use the existing
            // converted routine if it's already been converted, or recursively convert the
            // target routine before continuing.
            |name, rule_id| {
                #[allow(clippy::map_entry)] // TODO(https://fxbug.dev/407087136)
                if self.routine_types.contains_key(&name) {
                    self.expect_routine_resolved(&name, routine_type, rule_id)
                } else {
                    self.convert_routine(
                        namespace,
                        &name,
                        uninstalled_routines,
                        ip_version_strictness,
                    )
                }
            },
        )?;

        // Insert the resulting converted routine in the `core_uninstalled` state.
        let target = CoreUninstalledRoutine::new(rules, id);
        let uninstalled = match routine_type {
            RoutineType::Ip => &mut self.ip,
            RoutineType::Nat => &mut self.nat,
        };
        assert_matches!(
            uninstalled.insert(name.to_owned(), target.clone()),
            None,
            "this routine should not have been converted unless there is a cycle, and we \
            detect cycles"
        );
        assert_matches!(self.routine_types.insert(name.to_owned(), routine_type), None);

        Ok(target)
    }

    fn expect_routine_resolved(
        &self,
        name: &str,
        calling_routine_type: RoutineType,
        rule_id: fnet_filter_ext::RuleId,
    ) -> Result<CoreUninstalledRoutine<I>, CommitError> {
        let Self { ip, nat, routine_types } = self;

        // Rules can only jump to target routines that are the same type as the calling
        // routine (e.g. NAT to NAT).
        let target_routine_type = routine_types.get(name).expect("target should be resolved");
        if *target_routine_type != calling_routine_type {
            return Err(CommitError::RuleWithInvalidAction(rule_id));
        }

        let uninstalled = match calling_routine_type {
            RoutineType::Ip => &ip,
            RoutineType::Nat => &nat,
        };
        Ok(uninstalled.get(name).expect("uninstalled routine should already be converted").clone())
    }
}

#[derive(Clone)]
enum InstalledRoutineType {
    Ip(InstalledIpRoutine),
    Nat(InstalledNatRoutine),
}

#[derive(Clone)]
struct InstalledRoutine {
    name: String,
    routine_type: InstalledRoutineType,
    rules: BTreeMap<u32, Rule>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RoutineType {
    Ip,
    Nat,
}

#[derive(Debug, Clone)]
struct UninstalledRoutine {
    routine_type: RoutineType,
    rules: BTreeMap<u32, Rule>,
    id: usize,
}

/// Converts a controller's state to the equivalent `netstack3_core` state.
///
/// Because Core state is parameterized on IP version, this requires splitting
/// the state in each of the controller's namespaces into v4-specific and v6-
/// specific state.
pub(super) fn convert_to_core(
    namespaces: HashMap<fnet_filter_ext::NamespaceId, Namespace>,
) -> Result<(State<Ipv4>, State<Ipv6>), CommitError> {
    let (mut v4, mut v6) = (State::<Ipv4>::default(), State::<Ipv6>::default());

    for (id, namespace) in namespaces {
        let Namespace { domain, routines } = namespace;

        let (installed, uninstalled) = routines.into_iter().fold(
            (Vec::new(), HashMap::new()),
            |(mut installed, mut uninstalled), (name, routine)| {
                let Routine { routine_type, rules } = routine;
                match routine_type {
                    super::controller::RoutineType::Ip(IpRoutineType::Uninstalled(id)) => {
                        assert_matches!(
                            uninstalled.insert(
                                name,
                                UninstalledRoutine { routine_type: RoutineType::Ip, rules, id },
                            ),
                            None
                        );
                    }
                    super::controller::RoutineType::Nat(NatRoutineType::Uninstalled(id)) => {
                        assert_matches!(
                            uninstalled.insert(
                                name,
                                UninstalledRoutine { routine_type: RoutineType::Nat, rules, id },
                            ),
                            None
                        );
                    }
                    super::controller::RoutineType::Ip(IpRoutineType::Installed(installation)) => {
                        installed.push(InstalledRoutine {
                            name,
                            routine_type: InstalledRoutineType::Ip(installation),
                            rules,
                        })
                    }
                    super::controller::RoutineType::Nat(NatRoutineType::Installed(
                        installation,
                    )) => installed.push(InstalledRoutine {
                        name,
                        routine_type: InstalledRoutineType::Nat(installation),
                        rules,
                    }),
                }
                (installed, uninstalled)
            },
        );

        match domain {
            fnet_filter_ext::Domain::Ipv4 => {
                v4.add_namespace(
                    &id,
                    installed,
                    uninstalled,
                    // Because this is an IPv4-specific domain, any rule that includes an
                    // IP-specific matcher must also be IPv4-specific.
                    IpVersionStrictness::IpVersionMustMatchState,
                )?;
            }
            fnet_filter_ext::Domain::Ipv6 => {
                v6.add_namespace(
                    &id,
                    installed,
                    uninstalled,
                    // Because this is an IPv6-specific domain, any rule that includes an
                    // IP-specific matcher must also be IPv6-specific.
                    IpVersionStrictness::IpVersionMustMatchState,
                )?;
            }
            fnet_filter_ext::Domain::AllIp => {
                // Because this is an IP-agnostic domain, rules can be IPv4- or IPv6-specific,
                // but IP-version-specific rules will be omitted from the version of the state
                // that does not match the rule's version. For example, a rule with an IPv4
                // address matcher will not be installed in Core's IPv6 filtering state.
                v4.add_namespace(
                    &id,
                    installed.clone(),
                    uninstalled.clone(),
                    IpVersionStrictness::IpVersionCanDifferFromState,
                )?;
                v6.add_namespace(
                    &id,
                    installed,
                    uninstalled,
                    IpVersionStrictness::IpVersionCanDifferFromState,
                )?;
            }
        }
    }

    Ok((v4, v6))
}
