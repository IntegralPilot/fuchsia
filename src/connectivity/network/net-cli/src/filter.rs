// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::{BTreeMap, HashMap};
use std::pin::pin;
use std::str::FromStr as _;

use anyhow::{Error, anyhow};
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_fuchsia_net_filter_ext::{
    self as fnet_filter_ext, ControllerId, Domain, InstalledIpRoutine, InstalledNatRoutine, IpHook,
    NamespaceId, NatHook, Resource, ResourceId, RoutineId, RoutineType, Rule, RuleId, Update,
};
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_filter_ext_fdomain as fidl_fuchsia_net_filter_ext;
#[cfg(not(feature = "fdomain"))]
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_matchers_ext_fdomain as fnet_matchers_ext;
use flex_fuchsia_net_filter as fnet_filter;
use flex_fuchsia_net_root as fnet_root;

use crate::{NetCliDepsConnector, connect_with_context, opts};

/// A datatype storing the filtering resource state. Design inspired by nftables.
///
/// nftables: table -> chains -> rules
/// netstack: namespace -> routines -> rules
///
/// Note: The Netstack has an additional datatype, Controller at the top level. Each controller has
/// many namespaces.
#[derive(Debug, Default)]
pub struct FilteringResources {
    controllers: BTreeMap<ControllerId, BTreeMap<NamespaceId, Namespace>>,
}

impl FilteringResources {
    /// Gets an iterator over ControllerId, ordered alphabetically.
    pub fn controllers(&self) -> impl Iterator<Item = &ControllerId> {
        self.controllers.keys()
    }

    /// Gets Namespaces, given a ControllerId.
    pub fn namespaces(
        &self,
        controller_id: &ControllerId,
    ) -> Option<impl Iterator<Item = &Namespace>> {
        let namespaces = self.controllers.get(controller_id)?.values();

        Some(namespaces)
    }

    fn find_namespace_mut(
        &mut self,
        controller_id: &ControllerId,
        namespace_id: &NamespaceId,
    ) -> Option<&mut Namespace> {
        self.controllers.get_mut(controller_id)?.get_mut(namespace_id)
    }

    fn add_namespace(
        &mut self,
        controller_id: ControllerId,
        namespace: fidl_fuchsia_net_filter_ext::Namespace,
    ) -> Option<Resource> {
        let namespace = Namespace::new(namespace);
        let namespaces = self.controllers.entry(controller_id).or_default();

        namespaces
            .insert(namespace.id.clone(), namespace)
            .map(|namespace| Resource::Namespace(namespace.into()))
    }

    fn remove_namespace(
        &mut self,
        controller_id: &ControllerId,
        namespace_id: &NamespaceId,
    ) -> Option<Resource> {
        let namespaces = self.controllers.get_mut(controller_id)?;

        let removed_namespace = namespaces.remove(namespace_id)?;

        // If last namespace deleted, remove the controller.
        if namespaces.is_empty() {
            let _ = self
                .controllers
                .remove(controller_id)
                .expect("Attempted to delete controller but failed.");
        }

        Some(Resource::Namespace(removed_namespace.into()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Namespace {
    pub id: NamespaceId,
    pub domain: Domain,
    routines: HashMap<RoutineId, Routine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Ord, PartialOrd)]
pub struct RoutinePriority(pub i32);

#[derive(Debug, Default, PartialEq)]
pub struct IpRoutines {
    pub ingress: BTreeMap<RoutinePriority, Vec<Routine>>,
    pub local_ingress: BTreeMap<RoutinePriority, Vec<Routine>>,
    pub forwarding: BTreeMap<RoutinePriority, Vec<Routine>>,
    pub local_egress: BTreeMap<RoutinePriority, Vec<Routine>>,
    pub egress: BTreeMap<RoutinePriority, Vec<Routine>>,
    pub uninstalled: Vec<Routine>,
}

impl IpRoutines {
    pub fn has_installed_routines(&self) -> bool {
        !self.ingress.is_empty()
            || !self.local_ingress.is_empty()
            || !self.forwarding.is_empty()
            || !self.local_egress.is_empty()
            || !self.egress.is_empty()
    }

    pub fn has_uninstalled_routines(&self) -> bool {
        !self.uninstalled.is_empty()
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct NatRoutines {
    pub ingress: BTreeMap<RoutinePriority, Vec<Routine>>,
    pub local_ingress: BTreeMap<RoutinePriority, Vec<Routine>>,
    pub local_egress: BTreeMap<RoutinePriority, Vec<Routine>>,
    pub egress: BTreeMap<RoutinePriority, Vec<Routine>>,
    pub uninstalled: Vec<Routine>,
}

impl NatRoutines {
    pub fn has_installed_routines(&self) -> bool {
        !self.ingress.is_empty()
            || !self.local_ingress.is_empty()
            || !self.local_egress.is_empty()
            || !self.egress.is_empty()
    }

    pub fn has_uninstalled_routines(&self) -> bool {
        !self.uninstalled.is_empty()
    }
}

impl Namespace {
    fn new(namespace: fidl_fuchsia_net_filter_ext::Namespace) -> Self {
        Self { id: namespace.id, domain: namespace.domain, routines: HashMap::new() }
    }

    /// Gets routines in a namespace grouped by their RoutineType.
    pub fn routines(&self) -> (IpRoutines, NatRoutines) {
        let mut ip_routines = IpRoutines::default();
        let mut nat_routines = NatRoutines::default();

        self.routines.values().for_each(|routine| {
            let routine = routine.clone();
            match routine.routine_type {
                RoutineType::Nat(None) => nat_routines.uninstalled.push(routine),
                RoutineType::Ip(None) => ip_routines.uninstalled.push(routine),
                RoutineType::Nat(Some(InstalledNatRoutine { priority, hook })) => {
                    let routines_on_hook = match hook {
                        NatHook::Egress => &mut nat_routines.egress,
                        NatHook::LocalEgress => &mut nat_routines.local_egress,
                        NatHook::Ingress => &mut nat_routines.ingress,
                        NatHook::LocalIngress => &mut nat_routines.local_ingress,
                    };

                    let routine_priority = RoutinePriority(priority);
                    routines_on_hook.entry(routine_priority).or_default().push(routine)
                }
                RoutineType::Ip(Some(InstalledIpRoutine { priority, hook })) => {
                    let routines_on_hook = match hook {
                        IpHook::Egress => &mut ip_routines.egress,
                        IpHook::LocalEgress => &mut ip_routines.local_egress,
                        IpHook::Ingress => &mut ip_routines.ingress,
                        IpHook::LocalIngress => &mut ip_routines.local_ingress,
                        IpHook::Forwarding => &mut ip_routines.forwarding,
                    };

                    let routine_priority = RoutinePriority(priority);
                    routines_on_hook.entry(routine_priority).or_default().push(routine)
                }
            }
        });

        (ip_routines, nat_routines)
    }

    fn find_routine_mut(&mut self, routine_id: &RoutineId) -> Option<&mut Routine> {
        self.routines.get_mut(routine_id)
    }

    fn add_routine(&mut self, routine: fidl_fuchsia_net_filter_ext::Routine) -> Option<Resource> {
        let routine = Routine::new(routine);

        self.routines
            .insert(routine.id.clone(), routine)
            .map(|routine| Resource::Routine(routine.into()))
    }

    fn remove_routine(&mut self, routine_id: &RoutineId) -> Option<Resource> {
        self.routines.remove(routine_id).map(|routine| Resource::Routine(routine.into()))
    }
}

impl Into<fidl_fuchsia_net_filter_ext::Namespace> for Namespace {
    fn into(self) -> fidl_fuchsia_net_filter_ext::Namespace {
        fidl_fuchsia_net_filter_ext::Namespace { id: self.id, domain: self.domain }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Routine {
    pub id: RoutineId,
    pub routine_type: RoutineType,
    rules: BTreeMap<u32, Rule>,
}

impl Routine {
    fn new(routine: fidl_fuchsia_net_filter_ext::Routine) -> Self {
        Self { id: routine.id, routine_type: routine.routine_type, rules: BTreeMap::new() }
    }

    /// Gets an iterator over the rules in this routine, in order by rule's index.
    pub fn rules(&self) -> impl Iterator<Item = &Rule> {
        self.rules.values()
    }

    fn add_rule(&mut self, rule: Rule) -> Option<Resource> {
        self.rules
            .insert(rule.id.index, rule)
            .map(|existing_rule| Resource::Rule(existing_rule.into()))
    }

    fn remove_rule(&mut self, rule_id: &RuleId) -> Option<Resource> {
        self.rules.remove(&rule_id.index).map(|rule| Resource::Rule(rule.into()))
    }
}

impl Into<fidl_fuchsia_net_filter_ext::Routine> for Routine {
    fn into(self) -> fidl_fuchsia_net_filter_ext::Routine {
        fidl_fuchsia_net_filter_ext::Routine { id: self.id, routine_type: self.routine_type }
    }
}

impl Update for FilteringResources {
    fn add(&mut self, controller: ControllerId, resource: Resource) -> Option<Resource> {
        match resource {
            Resource::Namespace(namespace) => self.add_namespace(controller, namespace),
            Resource::Routine(routine) => {
                let namespace = self
                    .find_namespace_mut(&controller, &routine.id.namespace)
                    .expect("Routine's namespace does not exist.");

                namespace.add_routine(routine)
            }
            Resource::Rule(rule) => {
                let namespace = self
                    .find_namespace_mut(&controller, &rule.id.routine.namespace)
                    .expect("Rule's namespace does not exist.");

                let routine = namespace
                    .find_routine_mut(&rule.id.routine)
                    .expect("Rule's routine does not exist.");

                routine.add_rule(rule)
            }
        }
    }

    fn remove(&mut self, controller: ControllerId, resource: &ResourceId) -> Option<Resource> {
        match resource {
            ResourceId::Namespace(namespace_id) => self.remove_namespace(&controller, namespace_id),
            ResourceId::Routine(routine_id) => {
                let namespace = self.find_namespace_mut(&controller, &routine_id.namespace)?;

                namespace.remove_routine(routine_id)
            }
            ResourceId::Rule(rule_id) => {
                let namespace = self.find_namespace_mut(&controller, &rule_id.routine.namespace)?;
                let routine = namespace.find_routine_mut(&rule_id.routine)?;

                routine.remove_rule(rule_id)
            }
        }
    }
}

pub(super) async fn do_filter<C: NetCliDepsConnector, W: std::io::Write>(
    mut out: W,
    cmd: opts::filter::FilterEnum,
    connector: &C,
) -> Result<(), Error> {
    use opts::filter::{
        Accept, Action, Create, Domain, Drop, FilterEnum, Hook, Jump, List, Masquerade, Namespace,
        NamespaceId, PortRange, Redirect, Remove, Resource, ResourceId, Return, Routine, RoutineId,
        RoutineType, Rule, RuleId, TransparentProxy, TransportProtocolMatcher,
    };

    match cmd {
        FilterEnum::List(List {}) => {
            let state = connect_with_context::<fnet_filter::StateMarker, _>(connector).await?;
            let stream = fnet_filter_ext::event_stream_from_state(state)?;
            let mut stream = pin!(stream);
            let resources: FilteringResources =
                fnet_filter_ext::get_existing_resources(&mut stream).await?;

            for controller_id @ ControllerId(id) in resources.controllers() {
                writeln_scoped(
                    &mut out,
                    0,
                    format!("controller: \"{}\"", id).as_str(),
                    |out, padding| {
                        for namespace in resources.namespaces(controller_id).unwrap() {
                            do_filter_print_namespace(out, padding, namespace)?;
                        }

                        Ok(())
                    },
                )?;
            }
        }
        FilterEnum::Create(Create { controller, resource, idempotent }) => {
            let resource = match resource {
                Resource::Namespace(Namespace { name, domain }) => {
                    fnet_filter_ext::Resource::Namespace(fnet_filter_ext::Namespace {
                        id: fnet_filter_ext::NamespaceId(name),
                        domain: match domain {
                            Domain::All => fnet_filter_ext::Domain::AllIp,
                            Domain::Ipv4 => fnet_filter_ext::Domain::Ipv4,
                            Domain::Ipv6 => fnet_filter_ext::Domain::Ipv6,
                        },
                    })
                }
                Resource::Routine(Routine { namespace, name, type_, hook, priority }) => {
                    fnet_filter_ext::Resource::Routine(fnet_filter_ext::Routine {
                        id: fnet_filter_ext::RoutineId {
                            namespace: fnet_filter_ext::NamespaceId(namespace),
                            name,
                        },
                        routine_type: match type_ {
                            RoutineType::Ip => {
                                let installation = match (hook, priority) {
                                    (None, None) => None,
                                    (Some(hook), priority) => {
                                        Some(fnet_filter_ext::InstalledIpRoutine {
                                            hook: match hook {
                                                Hook::Ingress => fnet_filter_ext::IpHook::Ingress,
                                                Hook::LocalIngress => {
                                                    fnet_filter_ext::IpHook::LocalIngress
                                                }
                                                Hook::Forwarding => {
                                                    fnet_filter_ext::IpHook::Forwarding
                                                }
                                                Hook::LocalEgress => {
                                                    fnet_filter_ext::IpHook::LocalEgress
                                                }
                                                Hook::Egress => fnet_filter_ext::IpHook::Egress,
                                            },
                                            priority: priority.unwrap_or_default(),
                                        })
                                    }
                                    (None, Some(_)) => {
                                        return Err(anyhow!(
                                            "cannot specify priority without an installation hook"
                                        ));
                                    }
                                };
                                fnet_filter_ext::RoutineType::Ip(installation)
                            }
                            RoutineType::Nat => {
                                let installation = match (hook, priority) {
                                    (None, None) => None,
                                    (Some(hook), priority) => {
                                        Some(fnet_filter_ext::InstalledNatRoutine {
                                            hook: match hook {
                                                Hook::Ingress => fnet_filter_ext::NatHook::Ingress,
                                                Hook::LocalIngress => {
                                                    fnet_filter_ext::NatHook::LocalIngress
                                                }
                                                Hook::LocalEgress => {
                                                    fnet_filter_ext::NatHook::LocalEgress
                                                }
                                                Hook::Egress => fnet_filter_ext::NatHook::Egress,
                                                Hook::Forwarding => {
                                                    return Err(anyhow!(
                                                        "NAT routines do not run in forwarding hook"
                                                    ));
                                                }
                                            },
                                            priority: priority.unwrap_or_default(),
                                        })
                                    }
                                    (None, Some(_)) => {
                                        return Err(anyhow!(
                                            "cannot specify priority without an installation hook"
                                        ));
                                    }
                                };
                                fnet_filter_ext::RoutineType::Nat(installation)
                            }
                        },
                    })
                }
                Resource::Rule(Rule {
                    namespace,
                    routine,
                    index,
                    in_interface,
                    out_interface,
                    src_addr,
                    dst_addr,
                    transport_protocol,
                    src_port,
                    dst_port,
                    action,
                }) => fnet_filter_ext::Resource::Rule(fnet_filter_ext::Rule {
                    id: fnet_filter_ext::RuleId {
                        routine: fnet_filter_ext::RoutineId {
                            namespace: fnet_filter_ext::NamespaceId(namespace),
                            name: routine,
                        },
                        index,
                    },
                    matchers: {
                        fnet_filter_ext::Matchers {
                            in_interface: in_interface.map(Into::into),
                            out_interface: out_interface.map(Into::into),
                            src_addr: src_addr.map(Into::into),
                            dst_addr: dst_addr.map(Into::into),
                            transport_protocol: match transport_protocol {
                                None => {
                                    if src_port != None || dst_port != None {
                                        return Err(anyhow!(
                                            "must provide a transport protocol to match on port"
                                        ));
                                    }
                                    None
                                }
                                Some(matcher) => Some(match matcher {
                                    TransportProtocolMatcher::Tcp => {
                                        fnet_matchers_ext::TransportProtocol::Tcp {
                                            src_port: src_port.map(Into::into),
                                            dst_port: dst_port.map(Into::into),
                                        }
                                    }
                                    TransportProtocolMatcher::Udp => {
                                        fnet_matchers_ext::TransportProtocol::Udp {
                                            src_port: src_port.map(Into::into),
                                            dst_port: dst_port.map(Into::into),
                                        }
                                    }
                                    TransportProtocolMatcher::Icmp => {
                                        if src_port != None || dst_port != None {
                                            return Err(anyhow!(
                                                "cannot match on src or dst port for ICMP packets"
                                            ));
                                        }
                                        fnet_matchers_ext::TransportProtocol::Icmp
                                    }
                                    TransportProtocolMatcher::Icmpv6 => {
                                        if src_port != None || dst_port != None {
                                            return Err(anyhow!(
                                                "cannot match on src or dst port for ICMPv6 packets"
                                            ));
                                        }
                                        fnet_matchers_ext::TransportProtocol::Icmpv6
                                    }
                                }),
                            },
                            ..Default::default()
                        }
                    },
                    action: match action {
                        Action::Accept(Accept {}) => fnet_filter_ext::Action::Accept,
                        Action::Drop(Drop {}) => fnet_filter_ext::Action::Drop,
                        Action::Jump(Jump { target }) => fnet_filter_ext::Action::Jump(target),
                        Action::Return(Return {}) => fnet_filter_ext::Action::Return,
                        Action::TransparentProxy(TransparentProxy { addr, port }) => {
                            let tproxy = match (addr, port) {
                                (None, None) => {
                                    return Err(anyhow!(
                                        "transparent proxy must specify at least one of local \
                                            address and local port"
                                    ));
                                }
                                (Some(addr), None) => {
                                    let addr = fnet_ext::IpAddress::from_str(&addr)?.into();
                                    fnet_filter_ext::TransparentProxy::LocalAddr(addr)
                                }
                                (None, Some(port)) => {
                                    fnet_filter_ext::TransparentProxy::LocalPort(port)
                                }
                                (Some(addr), Some(port)) => {
                                    let addr = fnet_ext::IpAddress::from_str(&addr)?.into();
                                    fnet_filter_ext::TransparentProxy::LocalAddrAndPort(addr, port)
                                }
                            };
                            fnet_filter_ext::Action::TransparentProxy(tproxy)
                        }
                        Action::Redirect(Redirect { dst_port }) => {
                            fnet_filter_ext::Action::Redirect {
                                dst_port: dst_port
                                    .map(|PortRange(range)| fnet_filter_ext::PortRange(range)),
                            }
                        }
                        Action::Masquerade(Masquerade { src_port }) => {
                            fnet_filter_ext::Action::Masquerade {
                                src_port: src_port
                                    .map(|PortRange(range)| fnet_filter_ext::PortRange(range)),
                            }
                        }
                    },
                }),
            };
            let root = connect_with_context::<fnet_root::FilterMarker, _>(connector).await?;
            let mut controller = fnet_filter_ext::Controller::new_root(
                &root,
                &fnet_filter_ext::ControllerId(controller),
            )
            .await?;
            controller.push_changes(vec![fnet_filter_ext::Change::Create(resource)]).await?;
            if idempotent.unwrap_or(false) {
                controller.commit_idempotent().await?;
            } else {
                controller.commit().await?;
            }
        }
        FilterEnum::Remove(Remove { controller, resource, idempotent }) => {
            let id = match resource {
                ResourceId::Namespace(NamespaceId { name }) => {
                    fnet_filter_ext::ResourceId::Namespace(fnet_filter_ext::NamespaceId(name))
                }
                ResourceId::Routine(RoutineId { namespace, name }) => {
                    fnet_filter_ext::ResourceId::Routine(fnet_filter_ext::RoutineId {
                        namespace: fnet_filter_ext::NamespaceId(namespace),
                        name,
                    })
                }
                ResourceId::Rule(RuleId { namespace, routine, index }) => {
                    fnet_filter_ext::ResourceId::Rule(fnet_filter_ext::RuleId {
                        routine: fnet_filter_ext::RoutineId {
                            namespace: fnet_filter_ext::NamespaceId(namespace),
                            name: routine,
                        },
                        index,
                    })
                }
            };
            let root = connect_with_context::<fnet_root::FilterMarker, _>(connector).await?;
            let mut controller = fnet_filter_ext::Controller::new_root(
                &root,
                &fnet_filter_ext::ControllerId(controller),
            )
            .await?;
            controller.push_changes(vec![fnet_filter_ext::Change::Remove(id)]).await?;
            if idempotent.unwrap_or(false) {
                controller.commit_idempotent().await?;
            } else {
                controller.commit().await?;
            }
        }
    }
    Ok(())
}

fn do_filter_print_namespace<W: std::io::Write>(
    out: &mut W,
    padding: usize,
    namespace: &Namespace,
) -> Result<(), Error> {
    let Namespace { id, domain, .. } = namespace;

    writeln_scoped(out, padding, format!("namespace: \"{}\"", id.0).as_str(), |out, padding| {
        writeln!(out, "{:padding$}domain: {:?}", "", domain)?;

        let (ip_routines, nat_routines) = &namespace.routines();

        if ip_routines.has_installed_routines() {
            writeln_scoped(out, padding, "installed IP routines", |out, padding| {
                let IpRoutines {
                    ingress,
                    local_ingress,
                    forwarding,
                    local_egress,
                    egress,
                    uninstalled: _,
                } = ip_routines;
                for (label, routines) in [
                    ("INGRESS", ingress),
                    ("LOCAL INGRESS", local_ingress),
                    ("FORWARDING", forwarding),
                    ("LOCAL EGRESS", local_egress),
                    ("EGRESS", egress),
                ] {
                    do_filter_print_installed_routines(out, padding, label, routines)?;
                }

                Ok(())
            })?;
        }

        if ip_routines.has_uninstalled_routines() {
            writeln_scoped(out, padding, "uninstalled IP routines", |out, padding| {
                for routine in ip_routines.uninstalled.iter() {
                    writeln_scoped(out, padding, routine.id.name.as_str(), |out, padding| {
                        do_filter_print_rules(out, padding, routine.rules())
                    })?;
                }

                Ok(())
            })?;
        }

        if nat_routines.has_installed_routines() {
            let NatRoutines { ingress, local_ingress, local_egress, egress, uninstalled: _ } =
                nat_routines;
            writeln_scoped(out, padding, "installed NAT routines", |out, padding| {
                for (label, routines) in [
                    ("INGRESS", ingress),
                    ("LOCAL INGRESS", local_ingress),
                    ("LOCAL EGRESS", local_egress),
                    ("EGRESS", egress),
                ] {
                    do_filter_print_installed_routines(out, padding, label, routines)?;
                }

                Ok(())
            })?;
        }

        if nat_routines.has_uninstalled_routines() {
            writeln_scoped(out, padding, "uninstalled NAT routines", |out, padding| {
                for routine in nat_routines.uninstalled.iter() {
                    writeln_scoped(out, padding, routine.id.name.as_str(), |out, padding| {
                        do_filter_print_rules(out, padding, routine.rules())
                    })?;
                }

                Ok(())
            })?;
        }
        Ok(())
    })
}

fn do_filter_print_installed_routines<W: std::io::Write>(
    out: &mut W,
    padding: usize,
    label: &str,
    routines: &BTreeMap<RoutinePriority, Vec<Routine>>,
) -> Result<(), Error> {
    if routines.is_empty() {
        return Ok(());
    }

    writeln_scoped(out, padding, label, |out, padding| {
        for (RoutinePriority(priority), routines) in routines.iter() {
            for routine @ Routine { id, .. } in routines {
                writeln_scoped(
                    out,
                    padding,
                    format!("{}. {}", priority, id.name).as_str(),
                    |out, padding| do_filter_print_rules(out, padding, routine.rules()),
                )?;
            }
        }

        Ok(())
    })?;

    Ok(())
}

fn do_filter_print_rules<'a, W: std::io::Write>(
    out: &mut W,
    padding: usize,
    rules: impl Iterator<Item = &'a Rule>,
) -> Result<(), Error> {
    for rule in rules {
        writeln!(
            out,
            "{:padding$}{}. {:?} -> {:?}",
            "", rule.id.index, rule.matchers, rule.action
        )?;
    }

    Ok(())
}

fn writeln_scoped<W: std::io::Write>(
    out: &mut W,
    padding: usize,
    label: &str,
    inner_scope: impl Fn(&mut W, usize) -> Result<(), Error>,
) -> Result<(), Error> {
    writeln!(out, "{:padding$}{label} {{", "")?;

    inner_scope(out, padding + 4)?;

    writeln!(out, "{:padding$}}}", "")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use fidl_fuchsia_net_filter_ext::{Action, Matchers};
    use futures::TryStreamExt as _;
    use itertools::Itertools;
    use net_declare::fidl_subnet;
    use std::num::NonZeroU64;

    use super::*;
    use crate::testutil::TestConnector;

    /// Test helpers
    impl FilteringResources {
        fn find_namespace(
            &self,
            controller_id: &ControllerId,
            namespace_id: &NamespaceId,
        ) -> Option<&Namespace> {
            self.controllers.get(controller_id)?.get(namespace_id)
        }

        fn namespaces_fidl(
            &self,
            controller_id: ControllerId,
        ) -> Option<Vec<fidl_fuchsia_net_filter_ext::Namespace>> {
            let namespaces = self
                .namespaces(&controller_id)?
                .map(|namespace| namespace.clone().into())
                .collect();

            Some(namespaces)
        }

        fn routines(
            &self,
            controller_id: ControllerId,
            namespace_id: NamespaceId,
        ) -> Option<Vec<fidl_fuchsia_net_filter_ext::Routine>> {
            let routines = self
                .find_namespace(&controller_id, &namespace_id)?
                .routines
                .values()
                .sorted_by(|routine_a, routine_b| routine_b.priority().cmp(&routine_a.priority()))
                .map(|routine| routine.clone().into())
                .collect();

            Some(routines)
        }

        fn rules(
            &self,
            controller_id: ControllerId,
            namespace_id: NamespaceId,
            routine_id: RoutineId,
        ) -> Option<Vec<&Rule>> {
            self.find_namespace(&controller_id, &namespace_id)?
                .routines
                .get(&routine_id)
                .map(|routine| routine.rules.values().collect_vec())
        }
    }

    impl Routine {
        fn priority(&self) -> Option<i32> {
            match &self.routine_type {
                RoutineType::Ip(Some(ip)) => Some(ip.priority),
                RoutineType::Nat(Some(nat)) => Some(nat.priority),
                RoutineType::Ip(None) | RoutineType::Nat(None) => None,
            }
        }
    }

    fn test_controller_a() -> ControllerId {
        ControllerId(String::from("test-controller-a"))
    }

    fn test_controller_b() -> ControllerId {
        ControllerId(String::from("test-controller-b"))
    }

    fn test_controller_c() -> ControllerId {
        ControllerId(String::from("test-controller-c"))
    }

    fn test_namespace_a() -> fidl_fuchsia_net_filter_ext::Namespace {
        fidl_fuchsia_net_filter_ext::Namespace {
            id: NamespaceId(String::from("test-namespace-a")),
            domain: Domain::AllIp,
        }
    }

    fn test_namespace_resource_a() -> Resource {
        Resource::Namespace(test_namespace_a())
    }

    fn test_namespace_b() -> fidl_fuchsia_net_filter_ext::Namespace {
        fidl_fuchsia_net_filter_ext::Namespace {
            id: NamespaceId(String::from("test-namespace-b")),
            domain: Domain::AllIp,
        }
    }

    fn test_namespace_resource_b() -> Resource {
        Resource::Namespace(test_namespace_b())
    }

    fn test_routine_a() -> fidl_fuchsia_net_filter_ext::Routine {
        fidl_fuchsia_net_filter_ext::Routine {
            id: RoutineId {
                namespace: test_namespace_a().id,
                name: String::from("test-routine-a"),
            },
            routine_type: RoutineType::Ip(None),
        }
    }

    fn test_routine_with_priority(
        name: String,
        priority: i32,
    ) -> fidl_fuchsia_net_filter_ext::Routine {
        fidl_fuchsia_net_filter_ext::Routine {
            id: RoutineId { namespace: test_namespace_a().id, name },
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                priority,
                hook: IpHook::Ingress,
            })),
        }
    }

    fn test_routine_with_routine_type(
        name: String,
        routine_type: RoutineType,
    ) -> fidl_fuchsia_net_filter_ext::Routine {
        fidl_fuchsia_net_filter_ext::Routine {
            id: RoutineId { namespace: test_namespace_a().id, name },
            routine_type,
        }
    }

    fn test_routine_resource_a() -> Resource {
        Resource::Routine(test_routine_a())
    }

    fn test_routine_b() -> fidl_fuchsia_net_filter_ext::Routine {
        fidl_fuchsia_net_filter_ext::Routine {
            id: RoutineId {
                namespace: test_namespace_b().id,
                name: String::from("test-routine-b"),
            },
            routine_type: RoutineType::Ip(None),
        }
    }

    fn test_routine_resource_b() -> Resource {
        Resource::Routine(test_routine_b())
    }

    fn test_rule_a() -> Rule {
        Rule {
            id: RuleId { routine: test_routine_a().id, index: 1 },
            matchers: Matchers::default(),
            action: Action::Accept,
        }
    }

    fn test_rule_a_with_index(index: u32) -> Rule {
        Rule {
            id: RuleId { routine: test_routine_a().id, index },
            matchers: Matchers::default(),
            action: Action::Accept,
        }
    }

    fn test_rule_resource_a() -> Resource {
        Resource::Rule(test_rule_a())
    }

    #[test]
    fn add_namespace_not_existing() {
        let mut tree = FilteringResources::default();

        let result = tree.add(test_controller_a(), test_namespace_resource_a());

        assert_eq!(result, None);
        assert_eq!(tree.namespaces_fidl(test_controller_a()), Some(vec![test_namespace_a()]));
    }

    #[test]
    fn add_namespace_adds_controller() {
        let mut tree = FilteringResources::default();

        let _ = tree.add(test_controller_a(), test_namespace_resource_a());

        assert_eq!(tree.controllers().collect_vec(), vec![&test_controller_a()]);
    }

    #[test]
    fn controllers_ordered_alphabetically() {
        let mut tree = FilteringResources::default();

        let _ = tree.add(test_controller_b(), test_namespace_resource_a());
        let _ = tree.add(test_controller_c(), test_namespace_resource_a());
        let _ = tree.add(test_controller_a(), test_namespace_resource_a());

        assert_eq!(
            tree.controllers().collect_vec(),
            vec![&test_controller_a(), &test_controller_b(), &test_controller_c()]
        );
    }

    #[test]
    fn add_namespace_existing() {
        let mut tree = FilteringResources::default();
        let _ = tree.add(test_controller_a(), test_namespace_resource_a()); // already added...

        let result = tree.add(test_controller_a(), test_namespace_resource_a());

        assert_eq!(result, Some(test_namespace_resource_a()));
        assert_eq!(tree.namespaces_fidl(test_controller_a()), Some(vec![test_namespace_a()]));
    }

    #[test]
    fn add_namespace_multiple() {
        let mut tree = FilteringResources::default();

        let _ = tree.add(test_controller_a(), test_namespace_resource_a());
        let _ = tree.add(test_controller_a(), test_namespace_resource_b());

        assert_eq!(
            tree.namespaces_fidl(test_controller_a()),
            Some(vec![test_namespace_a(), test_namespace_b(),])
        );
    }

    #[test]
    fn add_namespace_scoped_to_controllers() {
        let mut tree = FilteringResources::default();

        let _ = tree.add(test_controller_a(), test_namespace_resource_a());
        let _ = tree.add(test_controller_b(), test_namespace_resource_a());

        assert_eq!(tree.namespaces_fidl(test_controller_a()), Some(vec![test_namespace_a()]));
        assert_eq!(tree.namespaces_fidl(test_controller_b()), Some(vec![test_namespace_a()]));
    }

    #[test]
    fn remove_namespace_not_existing() {
        let mut tree = FilteringResources::default();

        let result = tree.remove(test_controller_a(), &test_namespace_resource_a().id());

        assert_eq!(result, None);
        assert_eq!(tree.namespaces_fidl(test_controller_a()), None);
    }

    #[test]
    fn remove_namespace_existing() {
        let mut tree = FilteringResources::default();
        let _ = tree.add(test_controller_a(), test_namespace_resource_a());

        let result = tree.remove(test_controller_a(), &test_namespace_resource_a().id());

        assert_eq!(result, Some(test_namespace_resource_a()));
        assert_eq!(tree.namespaces_fidl(test_controller_a()), None);
    }

    #[test]
    fn remove_namespace_multiple() {
        let mut tree = FilteringResources::default();
        let _ = tree.add(test_controller_a(), test_namespace_resource_a());
        let _ = tree.add(test_controller_a(), test_namespace_resource_b());

        let result = tree.remove(test_controller_a(), &test_namespace_resource_a().id());

        assert_eq!(result, Some(test_namespace_resource_a()));
        assert_eq!(tree.namespaces_fidl(test_controller_a()), Some(vec![test_namespace_b(),]));
    }

    #[test]
    fn remove_namespace_scoped_to_controllers() {
        let mut tree = FilteringResources::default();

        let _ = tree.add(test_controller_a(), test_namespace_resource_a());
        let _ = tree.add(test_controller_b(), test_namespace_resource_a());

        let result = tree.remove(test_controller_a(), &test_namespace_resource_a().id());

        assert_eq!(result, Some(test_namespace_resource_a()));
        assert_eq!(tree.namespaces_fidl(test_controller_a()), None);
        assert_eq!(tree.namespaces_fidl(test_controller_b()), Some(vec![test_namespace_a()]));
    }

    #[test]
    fn remove_last_namespace_on_controller_removes_controller() {
        let mut tree = FilteringResources::default();
        let _ = tree.add(test_controller_a(), test_namespace_resource_a());
        let _ = tree.add(test_controller_a(), test_namespace_resource_b());

        let _ = tree.remove(test_controller_a(), &test_namespace_resource_a().id());
        assert_eq!(tree.controllers().collect_vec(), vec![&test_controller_a()]); // still 1 left

        let _ = tree.remove(test_controller_a(), &test_namespace_resource_b().id());
        // now empty
        assert_eq!(tree.controllers().collect_vec(), vec![] as Vec<&ControllerId>);
    }

    #[test]
    #[should_panic(expected = "Routine's namespace does not exist.")]
    fn add_routine_with_namespace_not_existing_panics() {
        let mut tree = FilteringResources::default();

        let _ = tree.add(test_controller_a(), test_routine_resource_a());
    }

    #[test]
    fn add_routine_with_namespace_existing() {
        let mut tree = FilteringResources::default();
        let _ = tree.add(test_controller_a(), test_namespace_resource_a());

        let result = tree.add(test_controller_a(), test_routine_resource_a());

        assert_eq!(result, None);
        assert_eq!(
            tree.routines(test_controller_a(), test_namespace_a().id),
            Some(vec![test_routine_a()])
        );
    }

    #[test]
    fn add_routine_with_routine_existing() {
        let mut tree = FilteringResources::default();
        let _ = tree.add(test_controller_a(), test_namespace_resource_a());
        let _ = tree.add(test_controller_a(), test_routine_resource_a());

        let result = tree.add(test_controller_a(), test_routine_resource_a());

        assert_eq!(result, Some(test_routine_resource_a()));
        assert_eq!(
            tree.routines(test_controller_a(), test_namespace_a().id),
            Some(vec![test_routine_a()])
        );
    }

    #[test]
    fn add_routine_multiple_different_namespaces() {
        let mut tree = FilteringResources::default();
        let _ = tree.add(test_controller_a(), test_namespace_resource_a());
        let _ = tree.add(test_controller_a(), test_namespace_resource_b());

        let _ = tree.add(test_controller_a(), test_routine_resource_a());
        let _ = tree.add(test_controller_a(), test_routine_resource_b());

        assert_eq!(
            tree.routines(test_controller_a(), test_namespace_a().id),
            Some(vec![test_routine_a()])
        );
        assert_eq!(
            tree.routines(test_controller_a(), test_namespace_b().id),
            Some(vec![test_routine_b()])
        );
    }

    #[test]
    fn add_routine_multiple_same_namespaces() {
        let mut tree = FilteringResources::default();
        let _ = tree.add(test_controller_a(), test_namespace_resource_a());

        let routine_a = test_routine_with_priority("test-routine-a".into(), 5);
        let routine_b = test_routine_with_priority("test-routine-b".into(), 4);
        let _ = tree.add(test_controller_a(), Resource::Routine(routine_a.clone()));
        let _ = tree.add(test_controller_a(), Resource::Routine(routine_b.clone()));

        assert_eq!(
            tree.routines(test_controller_a(), test_namespace_a().id),
            Some(vec![routine_a, routine_b])
        );
    }

    #[test]
    fn add_routines_generates_correct_nat_ip_data_structure() {
        let mut namespace = Namespace::new(test_namespace_a());
        let routine_priority = RoutinePriority(12);

        let ip_ingress = test_routine_with_routine_type(
            "routine-ip-ingress".to_string(),
            RoutineType::Ip(Some(InstalledIpRoutine {
                priority: routine_priority.0,
                hook: IpHook::Ingress,
            })),
        );
        let ip_local_ingress = test_routine_with_routine_type(
            "routine-ip-local-ingress".to_string(),
            RoutineType::Ip(Some(InstalledIpRoutine {
                priority: routine_priority.0,
                hook: IpHook::LocalIngress,
            })),
        );
        let ip_egress = test_routine_with_routine_type(
            "routine-ip-egress".to_string(),
            RoutineType::Ip(Some(InstalledIpRoutine {
                priority: routine_priority.0,
                hook: IpHook::Egress,
            })),
        );
        let ip_local_egress = test_routine_with_routine_type(
            "routine-ip-local-egress".to_string(),
            RoutineType::Ip(Some(InstalledIpRoutine {
                priority: routine_priority.0,
                hook: IpHook::LocalEgress,
            })),
        );
        let ip_forwarding = test_routine_with_routine_type(
            "routine-ip-forwarding".to_string(),
            RoutineType::Ip(Some(InstalledIpRoutine {
                priority: routine_priority.0,
                hook: IpHook::Forwarding,
            })),
        );
        let ip_uninstalled = test_routine_with_routine_type(
            "routine-ip-uninstalled".to_string(),
            RoutineType::Ip(None),
        );
        let nat_ingress = test_routine_with_routine_type(
            "routine-nat-ingress".to_string(),
            RoutineType::Nat(Some(InstalledNatRoutine {
                priority: routine_priority.0,
                hook: NatHook::Ingress,
            })),
        );
        let nat_local_ingress = test_routine_with_routine_type(
            "routine-nat-local-ingress".to_string(),
            RoutineType::Nat(Some(InstalledNatRoutine {
                priority: routine_priority.0,
                hook: NatHook::LocalIngress,
            })),
        );
        let nat_egress = test_routine_with_routine_type(
            "routine-nat-egress".to_string(),
            RoutineType::Nat(Some(InstalledNatRoutine {
                priority: routine_priority.0,
                hook: NatHook::Egress,
            })),
        );
        let nat_local_egress = test_routine_with_routine_type(
            "routine-nat-local-egress".to_string(),
            RoutineType::Nat(Some(InstalledNatRoutine {
                priority: routine_priority.0,
                hook: NatHook::LocalEgress,
            })),
        );
        let nat_uninstalled = test_routine_with_routine_type(
            "routine-nat-uninstalled".to_string(),
            RoutineType::Nat(None),
        );

        let _ = namespace.add_routine(ip_ingress.clone());
        let _ = namespace.add_routine(ip_local_ingress.clone());
        let _ = namespace.add_routine(ip_egress.clone());
        let _ = namespace.add_routine(ip_local_egress.clone());
        let _ = namespace.add_routine(ip_forwarding.clone());
        let _ = namespace.add_routine(ip_uninstalled.clone());
        let _ = namespace.add_routine(nat_ingress.clone());
        let _ = namespace.add_routine(nat_local_ingress.clone());
        let _ = namespace.add_routine(nat_egress.clone());
        let _ = namespace.add_routine(nat_local_egress.clone());
        let _ = namespace.add_routine(nat_uninstalled.clone());

        assert_eq!(
            namespace.routines(),
            (
                IpRoutines {
                    ingress: BTreeMap::from([(
                        routine_priority.clone(),
                        vec![Routine::new(ip_ingress)]
                    )]),
                    local_ingress: BTreeMap::from([(
                        routine_priority.clone(),
                        vec![Routine::new(ip_local_ingress)]
                    )]),
                    forwarding: BTreeMap::from([(
                        routine_priority.clone(),
                        vec![Routine::new(ip_forwarding)]
                    )]),
                    local_egress: BTreeMap::from([(
                        routine_priority.clone(),
                        vec![Routine::new(ip_local_egress)]
                    )]),
                    egress: BTreeMap::from([(
                        routine_priority.clone(),
                        vec![Routine::new(ip_egress)]
                    )]),
                    uninstalled: vec![Routine::new(ip_uninstalled)],
                },
                NatRoutines {
                    ingress: BTreeMap::from([(
                        routine_priority.clone(),
                        vec![Routine::new(nat_ingress)]
                    )]),
                    local_ingress: BTreeMap::from([(
                        routine_priority.clone(),
                        vec![Routine::new(nat_local_ingress)]
                    )]),
                    local_egress: BTreeMap::from([(
                        routine_priority.clone(),
                        vec![Routine::new(nat_local_egress)]
                    )]),
                    egress: BTreeMap::from([(
                        routine_priority.clone(),
                        vec![Routine::new(nat_egress)]
                    )]),
                    uninstalled: vec![Routine::new(nat_uninstalled)],
                },
            )
        );
    }

    #[test]
    fn add_routines_orders_by_priority() {
        let mut namespace = Namespace::new(test_namespace_a());

        let routine_first = test_routine_with_priority("routine-a".to_string(), 12);
        let routine_second = test_routine_with_priority("routine-b".to_string(), 13);
        let routine_third = test_routine_with_priority("routine-c".to_string(), 14);

        let _ = namespace.add_routine(routine_third.clone());
        let _ = namespace.add_routine(routine_first.clone());
        let _ = namespace.add_routine(routine_second.clone());

        let routines: Vec<fidl_fuchsia_net_filter_ext::Routine> = namespace
            .routines()
            .0
            .ingress
            .values()
            .flatten()
            .map(|routine| routine.clone().into())
            .collect();

        assert_eq!(routines, vec![routine_first, routine_second, routine_third]);
    }

    #[test]
    fn add_routines_handles_duplicate_priority() {
        const PRIORITY: i32 = 14;
        let mut namespace = Namespace::new(test_namespace_a());

        let routine_first = test_routine_with_priority("routine-a".to_string(), PRIORITY);
        let routine_second = test_routine_with_priority("routine-b".to_string(), PRIORITY);

        let _ = namespace.add_routine(routine_first.clone());
        let _ = namespace.add_routine(routine_second.clone());

        let routines: Vec<fidl_fuchsia_net_filter_ext::Routine> = namespace
            .routines()
            .0
            .ingress
            .values()
            .flatten()
            // Sort for deterministic testing.
            .sorted_by(|routine_a, routine_b| routine_a.id.name.cmp(&routine_b.id.name))
            .map(|routines| routines.clone().into())
            .collect();

        assert_eq!(routines, vec![routine_first, routine_second]);
    }

    #[test]
    fn remove_routine_not_existing() {
        let mut tree = FilteringResources::default();

        let result = tree.remove(test_controller_a(), &test_routine_resource_a().id());

        assert_eq!(result, None);
        assert_eq!(tree.routines(test_controller_a(), test_namespace_a().id), None);
    }

    #[test]
    fn remove_routine_existing() {
        let mut tree = FilteringResources::default();
        let _ = tree.add(test_controller_a(), test_namespace_resource_a());
        let _ = tree.add(test_controller_a(), test_routine_resource_a());

        let result = tree.remove(test_controller_a(), &test_routine_resource_a().id());

        assert_eq!(result, Some(test_routine_resource_a()));
        assert_eq!(tree.routines(test_controller_a(), test_namespace_a().id), Some(vec![]));
    }

    #[test]
    #[should_panic(expected = "Rule's namespace does not exist.")]
    fn add_rule_with_namespace_not_existing_panics() {
        let mut tree = FilteringResources::default();

        let _ = tree.add(test_controller_a(), test_rule_resource_a());
    }

    #[test]
    #[should_panic(expected = "Rule's routine does not exist.")]
    fn add_rule_with_routine_not_existing_panics() {
        let mut tree = FilteringResources::default();
        let _ = tree.add(test_controller_a(), test_namespace_resource_a());

        let _ = tree.add(test_controller_a(), test_rule_resource_a());
    }

    #[test]
    fn add_rule_with_namespace_and_routine_existing() {
        let mut tree = FilteringResources::default();
        let _ = tree.add(test_controller_a(), test_namespace_resource_a());
        let _ = tree.add(test_controller_a(), test_routine_resource_a());

        let result = tree.add(test_controller_a(), test_rule_resource_a());

        assert_eq!(result, None);
        assert_eq!(
            tree.rules(test_controller_a(), test_namespace_a().id, test_routine_a().id),
            Some(vec![&test_rule_a()])
        );
    }

    #[test]
    fn add_rule_with_routine_existing() {
        let mut tree = FilteringResources::default();
        let _ = tree.add(test_controller_a(), test_namespace_resource_a());
        let _ = tree.add(test_controller_a(), test_routine_resource_a());
        let _ = tree.add(test_controller_a(), test_rule_resource_a());

        let result = tree.add(test_controller_a(), test_rule_resource_a());

        assert_eq!(result, Some(test_rule_resource_a()));
        assert_eq!(
            tree.rules(test_controller_a(), test_namespace_a().id, test_routine_a().id),
            Some(vec![&test_rule_a()])
        );
    }

    #[test]
    fn add_rules_orders_by_index() {
        let mut routine = Routine::new(test_routine_a());

        let rule_first = test_rule_a_with_index(12);
        let rule_second = test_rule_a_with_index(13);
        let rule_third = test_rule_a_with_index(14);

        let _ = routine.add_rule(rule_third.clone());
        let _ = routine.add_rule(rule_first.clone());
        let _ = routine.add_rule(rule_second.clone());

        let rules: Vec<&Rule> = routine.rules().collect_vec();

        assert_eq!(rules, vec![&rule_first, &rule_second, &rule_third]);
    }

    #[test]
    fn remove_rule_not_existing() {
        let mut tree = FilteringResources::default();

        let result = tree.remove(test_controller_a(), &test_rule_resource_a().id());

        assert_eq!(result, None);
        assert_eq!(
            tree.rules(test_controller_a(), test_namespace_a().id, test_routine_a().id),
            None
        );
    }

    #[test]
    fn remove_rule_existing() {
        let mut tree = FilteringResources::default();
        let _ = tree.add(test_controller_a(), test_namespace_resource_a());
        let _ = tree.add(test_controller_a(), test_routine_resource_a());
        let _ = tree.add(test_controller_a(), test_rule_resource_a());

        let result = tree.remove(test_controller_a(), &test_rule_resource_a().id());

        assert_eq!(result, Some(test_rule_resource_a()));
        assert_eq!(
            tree.rules(test_controller_a(), test_namespace_a().id, test_routine_a().id),
            Some(vec![])
        );
    }

    #[fuchsia::test]
    async fn test_do_filter() {
        const CONTROLLER_A: &str = "controller a";
        const CONTROLLER_B: &str = "controller b";
        const NAMESPACE_A: &str = "namespace a";
        const NAMESPACE_B: &str = "namespace b";
        const NAMESPACE_C: &str = "namespace c";
        const ROUTINE_A: &str = "routine a";
        const ROUTINE_B: &str = "routine b";
        const ROUTINE_C: &str = "routine c";
        const ROUTINE_D: &str = "routine d";
        const ROUTINE_E: &str = "routine e";
        const INDEX_FIRST: u32 = 11;
        const INDEX_SECOND: u32 = 12;
        const INDEX_THIRD: u32 = 13;
        const INDEX_FOURTH: u32 = 14;

        let client = flex_local::local_client_empty();

        let mut output = Vec::new();
        let (filter, mut requests) = client.create_proxy_and_stream::<fnet_filter::StateMarker>();

        let connector = TestConnector { filter: Some(filter), ..Default::default() };
        let op = do_filter(
            &mut output,
            opts::filter::FilterEnum::List(opts::filter::List {}),
            &connector,
        );
        let op_succeeds = async move {
            let req = requests
                .try_next()
                .await
                .expect("receiving request")
                .expect("request stream should not have ended");
            let (_options, server_end, _state_control_handle) =
                req.into_get_watcher().expect("request should be of type GetWatcher");

            let mut watcher_request_stream = server_end.into_stream();

            let events = [
                // controller a, namespace a
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Namespace(fnet_filter_ext::Namespace {
                        id: fnet_filter_ext::NamespaceId(NAMESPACE_A.to_string()),
                        domain: fnet_filter_ext::Domain::Ipv4,
                    }),
                )
                .into(),
                // controller a, namespace b
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Namespace(fnet_filter_ext::Namespace {
                        id: fnet_filter_ext::NamespaceId(NAMESPACE_B.to_string()),
                        domain: fnet_filter_ext::Domain::Ipv4,
                    }),
                )
                .into(),
                // controller b, namespace c
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_B.to_string()),
                    fnet_filter_ext::Resource::Namespace(fnet_filter_ext::Namespace {
                        id: fnet_filter_ext::NamespaceId(NAMESPACE_C.to_string()),
                        domain: fnet_filter_ext::Domain::Ipv4,
                    }),
                )
                .into(),
                // controller a, namespace a, routine a (IP)
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Routine(fnet_filter_ext::Routine {
                        id: fnet_filter_ext::RoutineId {
                            namespace: fnet_filter_ext::NamespaceId(NAMESPACE_A.to_string()),
                            name: ROUTINE_A.to_string(),
                        },
                        routine_type: fnet_filter_ext::RoutineType::Ip(Some(InstalledIpRoutine {
                            priority: 2,
                            hook: fnet_filter_ext::IpHook::Egress,
                        })),
                    }),
                )
                .into(),
                // controller a, namespace a, routine b (IP)
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Routine(fnet_filter_ext::Routine {
                        id: fnet_filter_ext::RoutineId {
                            namespace: fnet_filter_ext::NamespaceId(NAMESPACE_A.to_string()),
                            name: ROUTINE_B.to_string(),
                        },
                        routine_type: fnet_filter_ext::RoutineType::Ip(Some(InstalledIpRoutine {
                            priority: 1,
                            hook: fnet_filter_ext::IpHook::Egress,
                        })),
                    }),
                )
                .into(),
                // controller a, namespace b, routine c (IP)
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Routine(fnet_filter_ext::Routine {
                        id: fnet_filter_ext::RoutineId {
                            namespace: fnet_filter_ext::NamespaceId(NAMESPACE_B.to_string()),
                            name: ROUTINE_C.to_string(),
                        },
                        routine_type: fnet_filter_ext::RoutineType::Ip(None),
                    }),
                )
                .into(),
                // controller a, namespace a, routine d (NAT)
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Routine(fnet_filter_ext::Routine {
                        id: fnet_filter_ext::RoutineId {
                            namespace: fnet_filter_ext::NamespaceId(NAMESPACE_A.to_string()),
                            name: ROUTINE_D.to_string(),
                        },
                        routine_type: fnet_filter_ext::RoutineType::Nat(Some(
                            InstalledNatRoutine {
                                priority: 1,
                                hook: fnet_filter_ext::NatHook::Egress,
                            },
                        )),
                    }),
                )
                .into(),
                // controller a, namespace b, routine e (NAT)
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Routine(fnet_filter_ext::Routine {
                        id: fnet_filter_ext::RoutineId {
                            namespace: fnet_filter_ext::NamespaceId(NAMESPACE_B.to_string()),
                            name: ROUTINE_E.to_string(),
                        },
                        routine_type: fnet_filter_ext::RoutineType::Nat(None),
                    }),
                )
                .into(),
                // controller a, namespace a, routine a, rule #1 (11)
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Rule(fnet_filter_ext::Rule {
                        id: fnet_filter_ext::RuleId {
                            routine: fnet_filter_ext::RoutineId {
                                namespace: fnet_filter_ext::NamespaceId(NAMESPACE_A.to_string()),
                                name: ROUTINE_A.to_string(),
                            },
                            index: INDEX_FIRST,
                        },
                        matchers: Default::default(),
                        action: fnet_filter_ext::Action::Accept,
                    }),
                )
                .into(),
                // controller a, namespace a, routine a, rule #2 (12)
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Rule(fnet_filter_ext::Rule {
                        id: fnet_filter_ext::RuleId {
                            routine: fnet_filter_ext::RoutineId {
                                namespace: fnet_filter_ext::NamespaceId(NAMESPACE_A.to_string()),
                                name: ROUTINE_A.to_string(),
                            },
                            index: INDEX_SECOND,
                        },
                        matchers: Default::default(),
                        action: fnet_filter_ext::Action::Accept,
                    }),
                )
                .into(),
                // controller a, namespace a, routine b, rule #3 (13)
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Rule(fnet_filter_ext::Rule {
                        id: fnet_filter_ext::RuleId {
                            routine: fnet_filter_ext::RoutineId {
                                namespace: fnet_filter_ext::NamespaceId(NAMESPACE_A.to_string()),
                                name: ROUTINE_B.to_string(),
                            },
                            index: INDEX_THIRD,
                        },
                        matchers: Default::default(),
                        action: fnet_filter_ext::Action::Accept,
                    }),
                )
                .into(),
                // controller a, namespace b, routine c, rule #1 (11)
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Rule(fnet_filter_ext::Rule {
                        id: fnet_filter_ext::RuleId {
                            routine: fnet_filter_ext::RoutineId {
                                namespace: fnet_filter_ext::NamespaceId(NAMESPACE_B.to_string()),
                                name: ROUTINE_C.to_string(),
                            },
                            index: INDEX_FIRST,
                        },
                        matchers: Default::default(),
                        action: fnet_filter_ext::Action::Accept,
                    }),
                )
                .into(),
                // Test Matcher output: InterfaceMatcher
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Rule(fnet_filter_ext::Rule {
                        id: fnet_filter_ext::RuleId {
                            routine: fnet_filter_ext::RoutineId {
                                namespace: fnet_filter_ext::NamespaceId(NAMESPACE_B.to_string()),
                                name: ROUTINE_E.to_string(),
                            },
                            index: INDEX_FIRST,
                        },
                        matchers: Matchers {
                            in_interface: Some(fnet_matchers_ext::Interface::Id(
                                NonZeroU64::new(23).expect("Failed to create NonZeroU64"),
                            )),
                            out_interface: None,
                            src_addr: None,
                            dst_addr: None,
                            transport_protocol: None,
                            ebpf_program: None,
                        },
                        action: fnet_filter_ext::Action::Accept,
                    }),
                )
                .into(),
                // Test Matcher output: AddressMatcher
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Rule(fnet_filter_ext::Rule {
                        id: fnet_filter_ext::RuleId {
                            routine: fnet_filter_ext::RoutineId {
                                namespace: fnet_filter_ext::NamespaceId(NAMESPACE_B.to_string()),
                                name: ROUTINE_E.to_string(),
                            },
                            index: INDEX_SECOND,
                        },
                        matchers: Matchers {
                            in_interface: None,
                            out_interface: None,
                            src_addr: Some(fnet_matchers_ext::Address {
                                matcher: fnet_matchers_ext::AddressMatcherType::Subnet(
                                    fidl_subnet!("192.168.0.1/32").try_into().unwrap(),
                                ),
                                invert: false,
                            }),
                            dst_addr: None,
                            transport_protocol: None,
                            ebpf_program: None,
                        },
                        action: fnet_filter_ext::Action::Accept,
                    }),
                )
                .into(),
                // Test Matcher output: TransportProtocolMatcher simple
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Rule(fnet_filter_ext::Rule {
                        id: fnet_filter_ext::RuleId {
                            routine: fnet_filter_ext::RoutineId {
                                namespace: fnet_filter_ext::NamespaceId(NAMESPACE_B.to_string()),
                                name: ROUTINE_E.to_string(),
                            },
                            index: INDEX_THIRD,
                        },
                        matchers: Matchers {
                            in_interface: None,
                            out_interface: None,
                            src_addr: None,
                            dst_addr: None,
                            transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Icmpv6),
                            ebpf_program: None,
                        },
                        action: fnet_filter_ext::Action::Accept,
                    }),
                )
                .into(),
                // Test Matcher output: TransportProtocolMatcher with src_port and dst_port
                fnet_filter_ext::Event::Existing(
                    fnet_filter_ext::ControllerId(CONTROLLER_A.to_string()),
                    fnet_filter_ext::Resource::Rule(fnet_filter_ext::Rule {
                        id: fnet_filter_ext::RuleId {
                            routine: fnet_filter_ext::RoutineId {
                                namespace: fnet_filter_ext::NamespaceId(NAMESPACE_B.to_string()),
                                name: ROUTINE_E.to_string(),
                            },
                            index: INDEX_FOURTH,
                        },
                        matchers: Matchers {
                            in_interface: None,
                            out_interface: None,
                            src_addr: None,
                            dst_addr: None,
                            transport_protocol: Some(fnet_matchers_ext::TransportProtocol::Tcp {
                                src_port: None,
                                dst_port: Some(
                                    fnet_matchers_ext::Port::new(41, 42, true)
                                        .expect("Failed to create PortMatcher"),
                                ),
                            }),
                            ebpf_program: None,
                        },
                        action: fnet_filter_ext::Action::Accept,
                    }),
                )
                .into(),
                fnet_filter_ext::Event::Idle.into(),
            ];

            watcher_request_stream
                .try_next()
                .await
                .expect("watcher watch FIDL error")
                .expect("watcher request stream should not have ended")
                .into_watch()
                .expect("request should be of type Watch")
                .send(&events)
                .expect("responder.send should succeed");

            assert_matches!(
                watcher_request_stream.try_next().await.expect("watcher watch FIDL error"),
                None,
                "remaining watcher request stream should be empty because client should close \
                watcher channel after observing existing resources"
            );

            Ok(())
        };
        let ((), ()) = futures::future::try_join(op, op_succeeds)
            .await
            .expect("filter server command should succeed");

        const WANT_OUTPUT: &str = r#"controller: "controller a" {
    namespace: "namespace a" {
        domain: Ipv4
        installed IP routines {
            EGRESS {
                1. routine b {
                    13. Matchers -> Accept
                }
                2. routine a {
                    11. Matchers -> Accept
                    12. Matchers -> Accept
                }
            }
        }
        installed NAT routines {
            EGRESS {
                1. routine d {
                }
            }
        }
    }
    namespace: "namespace b" {
        domain: Ipv4
        uninstalled IP routines {
            routine c {
                11. Matchers -> Accept
            }
        }
        uninstalled NAT routines {
            routine e {
                11. Matchers { in_interface: Id(23) } -> Accept
                12. Matchers { src_addr: Address { matcher: 192.168.0.1/32, invert: false } } -> Accept
                13. Matchers { transport_protocol: Icmpv6 } -> Accept
                14. Matchers { transport_protocol: Tcp { dst_port: Port { range: 41..=42, invert: true } } } -> Accept
            }
        }
    }
}
controller: "controller b" {
    namespace: "namespace c" {
        domain: Ipv4
    }
}
"#;
        let got_output = std::str::from_utf8(&output).unwrap();
        pretty_assertions::assert_eq!(got_output, WANT_OUTPUT,);
    }
}
