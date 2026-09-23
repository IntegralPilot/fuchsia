// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::{BTreeMap, HashMap, btree_map, hash_map};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use derivative::Derivative;
use fidl_fuchsia_ebpf as febpf;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_ext as fnet_filter_ext;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use net_types::ip::{Ipv4, Ipv6};

use super::CommitError;
use crate::bindings::BindingsCtx;
use crate::bindings::bpf::{SocketFilterProgram, ValidVerifiedProgram};

#[derive(Debug, Clone)]
pub(super) struct SocketFilterProgramWithId {
    pub id: febpf::ProgramId,
    pub program: Arc<SocketFilterProgram>,
}

impl PartialEq for SocketFilterProgramWithId {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

/// Equivalent of `fnet_filter_ext::Matchers` that keeps a reference to the
/// `ebpf_program` instead of just the ID.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Matchers {
    pub in_interface: Option<fnet_matchers_ext::Interface>,
    pub out_interface: Option<fnet_matchers_ext::Interface>,
    pub src_addr: Option<fnet_matchers_ext::Address>,
    pub dst_addr: Option<fnet_matchers_ext::Address>,
    pub transport_protocol: Option<fnet_matchers_ext::TransportProtocol>,
    pub ebpf_program: Option<SocketFilterProgramWithId>,
}

impl Matchers {
    fn new(
        matchers: fnet_filter_ext::Matchers,
        ebpf_registry: &EbpfProgramRegistry,
    ) -> Result<Self, fnet_filter::CommitError> {
        let fnet_filter_ext::Matchers {
            in_interface,
            out_interface,
            src_addr,
            dst_addr,
            transport_protocol,
            ebpf_program,
        } = matchers;

        let ebpf_program = ebpf_program
            .map(|id| {
                let program =
                    ebpf_registry.get(&id).ok_or(fnet_filter::CommitError::InvalidEbpfProgramId)?;
                Ok(SocketFilterProgramWithId { id, program })
            })
            .transpose()?;

        Ok(Self {
            in_interface,
            out_interface,
            src_addr,
            dst_addr,
            transport_protocol,
            ebpf_program,
        })
    }

    pub(super) fn to_fidl(&self) -> fnet_filter_ext::Matchers {
        fnet_filter_ext::Matchers {
            in_interface: self.in_interface.clone(),
            out_interface: self.out_interface.clone(),
            src_addr: self.src_addr.clone(),
            dst_addr: self.dst_addr.clone(),
            transport_protocol: self.transport_protocol.clone(),
            ebpf_program: self.ebpf_program.as_ref().map(|program| program.id),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Rule {
    pub matchers: Matchers,
    pub action: fnet_filter_ext::Action,
}

/// When there are two routines that are installed on the same hook with the
/// same priority, the routine that was installed earlier will be evaluated
/// first. This atomic counter is incremented on addition of each routine,
/// giving us a monotonically increasing value we can use to sort routines in
/// order of installation. It's also useful for providing a unique ID to Core
/// for each uninstalled routine.
static ROUTINE_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Derivative)]
#[derivative(PartialEq)]
pub(super) struct InstalledIpRoutine {
    pub hook: fnet_filter_ext::IpHook,
    pub priority: i32,
    #[derivative(PartialEq = "ignore")]
    pub installation_order: usize,
}

#[derive(Debug, Clone, Derivative)]
#[derivative(PartialEq)]
pub(super) struct InstalledNatRoutine {
    pub hook: fnet_filter_ext::NatHook,
    pub priority: i32,
    #[derivative(PartialEq = "ignore")]
    pub installation_order: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum IpRoutineType {
    Installed(InstalledIpRoutine),
    Uninstalled(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum NatRoutineType {
    Installed(InstalledNatRoutine),
    Uninstalled(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum RoutineType {
    Ip(IpRoutineType),
    Nat(NatRoutineType),
}

impl RoutineType {
    pub fn is_installed(&self) -> bool {
        // The `InstalledIpRoutine` or `InstalledNatRoutine` configuration is
        // optional, and when omitted, signifies an uninstalled routine.
        match self {
            Self::Ip(IpRoutineType::Installed(_)) | Self::Nat(NatRoutineType::Installed(_)) => true,
            Self::Ip(IpRoutineType::Uninstalled(_)) | Self::Nat(NatRoutineType::Uninstalled(_)) => {
                false
            }
        }
    }
}

impl From<fnet_filter_ext::RoutineType> for RoutineType {
    fn from(routine_type: fnet_filter_ext::RoutineType) -> Self {
        match routine_type {
            fnet_filter_ext::RoutineType::Ip(installation) => Self::Ip(installation.map_or_else(
                || IpRoutineType::Uninstalled(ROUTINE_COUNTER.fetch_add(1, Ordering::Relaxed)),
                |fnet_filter_ext::InstalledIpRoutine { hook, priority }| {
                    IpRoutineType::Installed(InstalledIpRoutine {
                        hook,
                        priority,
                        installation_order: ROUTINE_COUNTER.fetch_add(1, Ordering::Relaxed),
                    })
                },
            )),
            fnet_filter_ext::RoutineType::Nat(installation) => Self::Nat(installation.map_or_else(
                || NatRoutineType::Uninstalled(ROUTINE_COUNTER.fetch_add(1, Ordering::Relaxed)),
                |fnet_filter_ext::InstalledNatRoutine { hook, priority }| {
                    NatRoutineType::Installed(InstalledNatRoutine {
                        hook,
                        priority,
                        installation_order: ROUTINE_COUNTER.fetch_add(1, Ordering::Relaxed),
                    })
                },
            )),
        }
    }
}

impl From<&RoutineType> for fnet_filter_ext::RoutineType {
    fn from(routine_type: &RoutineType) -> Self {
        match routine_type {
            RoutineType::Ip(routine_type) => Self::Ip(match routine_type {
                IpRoutineType::Uninstalled(_) => None,
                IpRoutineType::Installed(InstalledIpRoutine {
                    hook,
                    priority,
                    installation_order: _,
                }) => {
                    Some(fnet_filter_ext::InstalledIpRoutine { hook: *hook, priority: *priority })
                }
            }),
            RoutineType::Nat(routine_type) => Self::Nat(match routine_type {
                NatRoutineType::Uninstalled(_) => None,
                NatRoutineType::Installed(InstalledNatRoutine {
                    hook,
                    priority,
                    installation_order: _,
                }) => {
                    Some(fnet_filter_ext::InstalledNatRoutine { hook: *hook, priority: *priority })
                }
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Routine {
    pub routine_type: RoutineType,
    pub rules: BTreeMap<u32, Rule>,
}

impl Routine {
    fn removal_events(self, id: fnet_filter_ext::RoutineId) -> impl Iterator<Item = Event> {
        let Self { rules, routine_type: _ } = self;
        let routine_id = id.clone();
        rules
            .into_keys()
            .map(move |index| {
                Event::Removed(fnet_filter_ext::ResourceId::Rule(fnet_filter_ext::RuleId {
                    routine: routine_id.clone(),
                    index,
                }))
            })
            .chain(std::iter::once(Event::Removed(fnet_filter_ext::ResourceId::Routine(id))))
    }
}

#[derive(Debug, Clone)]
pub(super) struct Namespace {
    pub domain: fnet_filter_ext::Domain,
    pub routines: HashMap<String, Routine>,
}

impl Namespace {
    fn removal_events(self, id: fnet_filter_ext::NamespaceId) -> impl Iterator<Item = Event> {
        let Self { routines, domain: _ } = self;
        let namespace_id = id.clone();
        routines
            .into_iter()
            .flat_map(move |(routine_id, routine)| {
                let routine_id = fnet_filter_ext::RoutineId {
                    namespace: namespace_id.clone(),
                    name: routine_id,
                };
                routine.removal_events(routine_id)
            })
            .chain(std::iter::once(Event::Removed(fnet_filter_ext::ResourceId::Namespace(id))))
    }
}

#[derive(Debug, Default)]
pub(crate) struct Controller {
    namespaces: HashMap<fnet_filter_ext::NamespaceId, Namespace>,
    // This converted state is retained by the `Controller` to make easier to
    // merge this controller's state with that of another that is being updated,
    // without having to re-do all the conversion work for each controller
    // whenever any of them is updated.
    //
    // It is always updated atomically with `namespaces`.
    pub core_state_v4: super::conversion::State<Ipv4>,
    pub core_state_v6: super::conversion::State<Ipv6>,
}

#[derive(Clone, Debug)]
pub(crate) enum Event {
    Added(fnet_filter_ext::Resource),
    Removed(fnet_filter_ext::ResourceId),
}

#[derive(Default, Debug)]
pub(super) struct EbpfProgramRegistry {
    // Programs that are registered with the `Controller`.
    programs: HashMap<febpf::ProgramId, Arc<SocketFilterProgram>>,
}

impl EbpfProgramRegistry {
    pub fn new() -> Self {
        Self { programs: HashMap::new() }
    }

    pub(super) fn register(
        &mut self,
        ctx: &BindingsCtx,
        id: febpf::ProgramId,
        program: ValidVerifiedProgram,
    ) -> Result<(), fnet_filter::RegisterEbpfProgramError> {
        match self.programs.entry(id) {
            hash_map::Entry::Occupied(_) => {
                Err(fnet_filter::RegisterEbpfProgramError::AlreadyRegistered)
            }
            hash_map::Entry::Vacant(entry) => {
                let program =
                    Arc::new(SocketFilterProgram::new(program, ctx.ebpf_manager.maps_cache())?);
                let _: &Arc<SocketFilterProgram> = entry.insert(program);
                Ok(())
            }
        }
    }

    pub(super) fn remove(&mut self, id: febpf::ProgramId) {
        let removed = self.programs.remove(&id);
        assert!(removed.is_some());
    }

    pub(super) fn get(&self, id: &febpf::ProgramId) -> Option<Arc<SocketFilterProgram>> {
        self.programs.get(id).cloned()
    }
}

pub(crate) struct CommitResult {
    pub events: Vec<Event>,
    pub new_state: HashMap<fnet_filter_ext::NamespaceId, Namespace>,
    pub core_state_v4: super::conversion::State<Ipv4>,
    pub core_state_v6: super::conversion::State<Ipv6>,
}

impl Controller {
    pub(crate) fn existing_ids(&self) -> impl Iterator<Item = fnet_filter_ext::ResourceId> + '_ {
        self.namespaces.iter().flat_map(|(namespace_id, Namespace { domain: _, routines })| {
            let namespace = fnet_filter_ext::ResourceId::Namespace(namespace_id.clone());
            let routines =
                routines.iter().flat_map(|(routine_id, Routine { routine_type: _, rules })| {
                    let routine =
                        fnet_filter_ext::ResourceId::Routine(fnet_filter_ext::RoutineId {
                            namespace: namespace_id.clone(),
                            name: routine_id.clone(),
                        });
                    let rules = rules.keys().map(|index| {
                        fnet_filter_ext::ResourceId::Rule(fnet_filter_ext::RuleId {
                            routine: fnet_filter_ext::RoutineId {
                                namespace: namespace_id.clone(),
                                name: routine_id.clone(),
                            },
                            index: *index,
                        })
                    });
                    std::iter::once(routine).chain(rules)
                });
            std::iter::once(namespace).chain(routines)
        })
    }

    pub(crate) fn existing_resources(
        &self,
    ) -> impl Iterator<Item = fnet_filter_ext::Resource> + '_ {
        self.namespaces.iter().flat_map(|(namespace_id, Namespace { domain, routines })| {
            let namespace = fnet_filter_ext::Resource::Namespace(fnet_filter_ext::Namespace {
                id: namespace_id.clone(),
                domain: domain.clone(),
            });
            let routines =
                routines.iter().flat_map(|(routine_id, Routine { routine_type, rules })| {
                    let routine = fnet_filter_ext::Resource::Routine(fnet_filter_ext::Routine {
                        id: fnet_filter_ext::RoutineId {
                            namespace: namespace_id.clone(),
                            name: routine_id.clone(),
                        },
                        routine_type: routine_type.into(),
                    });
                    let rules = rules.iter().map(|(index, Rule { matchers, action })| {
                        fnet_filter_ext::Resource::Rule(fnet_filter_ext::Rule {
                            id: fnet_filter_ext::RuleId {
                                routine: fnet_filter_ext::RoutineId {
                                    namespace: namespace_id.clone(),
                                    name: routine_id.clone(),
                                },
                                index: *index,
                            },
                            matchers: matchers.to_fidl(),
                            action: action.clone(),
                        })
                    });
                    std::iter::once(routine).chain(rules)
                });
            std::iter::once(namespace).chain(routines)
        })
    }

    pub(crate) fn validate_and_convert_changes(
        &self,
        changes: Vec<fnet_filter_ext::Change>,
        idempotent: bool,
        ebpf_registry: &EbpfProgramRegistry,
    ) -> Result<CommitResult, CommitError> {
        let validator = Validator::new(self.namespaces.clone());
        let (new_state, events) = validator.validate(changes, idempotent, ebpf_registry)?;

        let (v4, v6) = super::conversion::convert_to_core(new_state.clone())?;

        Ok(CommitResult { events, new_state, core_state_v4: v4, core_state_v6: v6 })
    }

    pub(crate) fn apply_new_state(
        &mut self,
        new_state: HashMap<fnet_filter_ext::NamespaceId, Namespace>,
        v4: super::conversion::State<Ipv4>,
        v6: super::conversion::State<Ipv6>,
    ) {
        self.namespaces = new_state;
        self.core_state_v4 = v4;
        self.core_state_v6 = v6;
    }
}

struct Validator {
    namespaces: HashMap<fnet_filter_ext::NamespaceId, Namespace>,
}

impl Validator {
    fn new(namespaces: HashMap<fnet_filter_ext::NamespaceId, Namespace>) -> Self {
        Self { namespaces }
    }

    fn validate(
        mut self,
        changes: Vec<fnet_filter_ext::Change>,
        idempotent: bool,
        ebpf_registry: &EbpfProgramRegistry,
    ) -> Result<(HashMap<fnet_filter_ext::NamespaceId, Namespace>, Vec<Event>), CommitError> {
        let mut events = Vec::new();
        for (i, change) in changes.into_iter().enumerate() {
            match change {
                fnet_filter_ext::Change::Create(resource) => {
                    self.add_resource(resource, idempotent, ebpf_registry).map(|event| {
                        if let Some(event) = event {
                            events.push(event);
                        }
                    })
                }
                fnet_filter_ext::Change::Remove(id) => {
                    self.remove_resource(id, idempotent).map(|removals| events.extend(removals))
                }
            }
            .map_err(|e| CommitError::ErrorOnChange { index: i, error: e })?;
        }
        let Self { namespaces } = self;
        Ok((namespaces, events))
    }

    fn add_resource(
        &mut self,
        resource: fnet_filter_ext::Resource,
        idempotent: bool,
        ebpf_registry: &EbpfProgramRegistry,
    ) -> Result<Option<Event>, fnet_filter::CommitError> {
        match resource.clone() {
            fnet_filter_ext::Resource::Namespace(fnet_filter_ext::Namespace { id, domain }) => {
                match self.namespaces.entry(id) {
                    hash_map::Entry::Vacant(entry) => {
                        let _ = entry.insert(Namespace { domain, routines: HashMap::new() });
                    }
                    hash_map::Entry::Occupied(entry) => {
                        // Note that if `idempotent` is set, we allow a namespace creation to
                        // succeed when that namespace already exists, as long as it has the same
                        // `domain` as the one being added. This is true even if the existing
                        // namespace already has routines configured.
                        if idempotent && entry.get().domain == domain {
                            return Ok(None);
                        } else {
                            return Err(fnet_filter::CommitError::AlreadyExists);
                        }
                    }
                }
            }
            fnet_filter_ext::Resource::Routine(fnet_filter_ext::Routine { id, routine_type }) => {
                let fnet_filter_ext::RoutineId { namespace, name } = id;
                let namespace = self
                    .namespaces
                    .get_mut(&namespace)
                    .ok_or(fnet_filter::CommitError::NamespaceNotFound)?;
                let routine_type = routine_type.into();
                match namespace.routines.entry(name) {
                    hash_map::Entry::Vacant(entry) => {
                        let _ = entry.insert(Routine { routine_type, rules: BTreeMap::default() });
                    }
                    hash_map::Entry::Occupied(entry) => {
                        // Note that if `idempotent` is set, we allow a routine creation to succeed
                        // when that routine already exists, as long as it has the same
                        // `routine_type` as the one being added. This is true even if the existing
                        // routine already has rules added to it.
                        if idempotent && entry.get().routine_type == routine_type {
                            return Ok(None);
                        } else {
                            return Err(fnet_filter::CommitError::AlreadyExists);
                        }
                    }
                }
            }
            fnet_filter_ext::Resource::Rule(rule) => {
                let fnet_filter_ext::Rule { id, matchers, action } = rule;
                let fnet_filter_ext::RuleId { routine, index } = id;
                let fnet_filter_ext::RoutineId { namespace, name: routine } = routine;

                let namespace = self
                    .namespaces
                    .get_mut(&namespace)
                    .ok_or(fnet_filter::CommitError::NamespaceNotFound)?;

                let matchers = Matchers::new(matchers, ebpf_registry)?;

                match &action {
                    fnet_filter_ext::Action::Jump(target) => {
                        let routine = namespace
                            .routines
                            .get(target)
                            .ok_or(fnet_filter::CommitError::RoutineNotFound)?;
                        if routine.routine_type.is_installed() {
                            return Err(fnet_filter::CommitError::TargetRoutineIsInstalled);
                        }
                    }
                    fnet_filter_ext::Action::Accept
                    | fnet_filter_ext::Action::Drop
                    | fnet_filter_ext::Action::Return
                    | fnet_filter_ext::Action::TransparentProxy(_)
                    | fnet_filter_ext::Action::Redirect { .. }
                    | fnet_filter_ext::Action::Masquerade { .. }
                    | fnet_filter_ext::Action::Mark { .. }
                    | fnet_filter_ext::Action::None
                    | fnet_filter_ext::Action::Reject(_) => {}
                }

                let to_insert = Rule { matchers, action };
                match namespace
                    .routines
                    .get_mut(&routine)
                    .ok_or(fnet_filter::CommitError::RoutineNotFound)?
                    .rules
                    .entry(index)
                {
                    btree_map::Entry::Vacant(entry) => {
                        let _ = entry.insert(to_insert);
                    }
                    btree_map::Entry::Occupied(entry) => {
                        // Note that if `idempotent` is set, we allow a rule creation to succeed
                        // when that routine already exists, as long as it has the same properties
                        // (matcher and action) as the one being added.
                        if idempotent && entry.get() == &to_insert {
                            return Ok(None);
                        } else {
                            return Err(fnet_filter::CommitError::AlreadyExists);
                        }
                    }
                }
            }
        }

        Ok(Some(Event::Added(resource)))
    }

    fn remove_resource(
        &mut self,
        id: fnet_filter_ext::ResourceId,
        idempotent: bool,
    ) -> Result<Vec<Event>, fnet_filter::CommitError> {
        let not_found_result = |err| if !idempotent { Err(err) } else { Ok(Vec::new()) };

        match id {
            fnet_filter_ext::ResourceId::Namespace(id) => match self.namespaces.entry(id.clone()) {
                hash_map::Entry::Vacant(_) => {
                    not_found_result(fnet_filter::CommitError::NamespaceNotFound)
                }
                hash_map::Entry::Occupied(entry) => {
                    let namespace = entry.remove();
                    Ok(namespace.removal_events(id).collect())
                }
            },
            fnet_filter_ext::ResourceId::Routine(id) => {
                let fnet_filter_ext::RoutineId { namespace, name } = &id;
                let Some(namespace) = self.namespaces.get_mut(namespace) else {
                    return not_found_result(fnet_filter::CommitError::NamespaceNotFound);
                };
                match namespace.routines.entry(name.clone()) {
                    hash_map::Entry::Vacant(_) => {
                        not_found_result(fnet_filter::CommitError::RoutineNotFound)
                    }
                    hash_map::Entry::Occupied(entry) => {
                        let routine = entry.remove();
                        Ok(routine.removal_events(id).collect())
                    }
                }
            }
            fnet_filter_ext::ResourceId::Rule(id) => {
                let fnet_filter_ext::RuleId { routine, index } = &id;
                let fnet_filter_ext::RoutineId { namespace, name: routine } = routine;
                let Some(namespace) = self.namespaces.get_mut(namespace) else {
                    return not_found_result(fnet_filter::CommitError::NamespaceNotFound);
                };
                let Some(routine) = namespace.routines.get_mut(routine) else {
                    return not_found_result(fnet_filter::CommitError::RoutineNotFound);
                };
                match routine.rules.entry(*index) {
                    btree_map::Entry::Vacant(_) => {
                        not_found_result(fnet_filter::CommitError::RuleNotFound)
                    }
                    btree_map::Entry::Occupied(entry) => {
                        let _ = entry.remove();
                        Ok(vec![Event::Removed(fnet_filter_ext::ResourceId::Rule(id))])
                    }
                }
            }
        }
    }
}
