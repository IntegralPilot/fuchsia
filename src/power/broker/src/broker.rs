// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::inspect::{
    AddElementInspectWriter, EagerInspectWriter, InspectAddDependency, InspectUpdateLevel,
    UpdateLevelInspectWriter,
};
use anyhow::{Context, Error, anyhow};
use async_utils::hanging_get::server::{HangingGet, Publisher, Subscriber};
use fidl_fuchsia_power_broker::{
    self as fpb, LeaseStatus, Permissions, RegisterDependencyTokenError, StatusError,
    StatusWatchPowerLevelResponder, UnregisterDependencyTokenError,
};
use fuchsia_inspect::{InspectType as IType, Node as INode};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use itertools::Itertools;
use std::borrow::Cow;
use std::cmp::max;
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::fmt::{self, Debug};
use std::hash::Hash;
use std::ops;
use uuid::Uuid;

use crate::credentials::*;
use crate::fpb::PowerLevel;
use crate::topology::*;

/// Max value for inspect event history.
const INSPECT_GRAPH_EVENT_BUFFER_SIZE: usize = 16384;

// Below are a series of type aliases for convenience
type LevelHangingGet<T> =
    HangingGet<IndexedPowerLevel, T, Box<dyn Fn(&IndexedPowerLevel, T) -> bool>>;
type LevelSubscriber<T> =
    Subscriber<IndexedPowerLevel, T, Box<dyn Fn(&IndexedPowerLevel, T) -> bool>>;
pub type CurrentLevelSubscriber = LevelSubscriber<StatusWatchPowerLevelResponder>;

/// An internal power level map used to describe the special case of lease elements,
/// which can only either be pending or satisfied.
#[repr(u8)]
enum LeasePowerLevel {
    Pending = PowerLevel::MIN,
    Satisfied = PowerLevel::MAX,
}

trait Responder<T> {
    type Error;
    fn send(responder: T, result: Result<u8, Self::Error>) -> Result<(), fidl::Error>;
}

impl Responder<StatusWatchPowerLevelResponder> for StatusWatchPowerLevelResponder {
    type Error = StatusError;
    fn send(
        responder: StatusWatchPowerLevelResponder,
        result: Result<u8, StatusError>,
    ) -> Result<(), fidl::Error> {
        responder.send(result)
    }
}

struct LevelAdmin<T> {
    /// We pass new power level values to the publisher, which takes care of updating the remote
    /// clients using hanging-gets.
    publisher: Publisher<IndexedPowerLevel, T, Box<dyn Fn(&IndexedPowerLevel, T) -> bool>>,
    /// We use this to vend a new subscriber for each new watch request stream.
    hanging_get: HangingGet<IndexedPowerLevel, T, Box<dyn Fn(&IndexedPowerLevel, T) -> bool>>,
    /// Cached `IndexedPowerLevel` value. Simply used to determine if the value has changed.
    level: IndexedPowerLevel,
}

impl<T: Responder<T>> LevelAdmin<T> {
    fn new(initial_level: IndexedPowerLevel) -> Self {
        let hanging_get: HangingGet<
            IndexedPowerLevel,
            T,
            Box<dyn Fn(&IndexedPowerLevel, T) -> bool>,
        > = LevelHangingGet::<T>::new(
            initial_level,
            Box::new(|level: &IndexedPowerLevel, res: T| -> bool {
                if let Err(error) = T::send(res, Ok(level.level)).context("response failed") {
                    log::warn!(error:?; "Failed to send power level to client");
                }
                true
            }),
        );
        let publisher = hanging_get.new_publisher();
        LevelAdmin::<T> { publisher, hanging_get, level: initial_level }
    }
}

pub struct Broker {
    catalog: Catalog,
    credentials: Registry,
    // The current level for each element, as reported to the broker.
    current: HashMap<ElementID, LevelAdmin<StatusWatchPowerLevelResponder>>,
    // The level each element is transitioning to, if it is transitioning.
    in_transition: HashMap<ElementID, IndexedPowerLevel>,
    // The level for each element required by the topology.
    required: SubscribeMap<ElementID, IndexedPowerLevel>,
    lease_counter: HashMap<ElementID, HashMap<PowerLevel, i64>>,
    _inspect_node: INode,
}

impl Broker {
    pub fn new(inspect: INode) -> Self {
        Broker {
            catalog: Catalog::new(&inspect),
            credentials: Registry::new(),
            current: HashMap::new(),
            in_transition: HashMap::new(),
            required: SubscribeMap::new(None),
            lease_counter: HashMap::new(),
            _inspect_node: inspect,
        }
    }

    pub fn lookup_name(&self, element_id: ElementID) -> Cow<'_, str> {
        self.catalog.topology.element_name(element_id)
    }

    fn lookup_credentials(&self, token: &Token) -> Option<Credential> {
        self.credentials.lookup(token)
    }

    fn unregister_all_credentials_for_element(&mut self, element_id: ElementID) {
        self.credentials.unregister_all_for_element(element_id)
    }

    #[cfg(test)]
    pub fn get_unsatisfiable_element_id(&self) -> ElementID {
        self.catalog.topology.get_unsatisfiable_element_id()
    }

    #[cfg(test)]
    pub fn get_unsatisfiable_element_name(&self) -> String {
        self.catalog.topology.get_unsatisfiable_element_name()
    }

    #[cfg(test)]
    pub fn get_unsatisfiable_element_levels(&self) -> Vec<u64> {
        self.catalog.topology.get_unsatisfiable_element_levels()
    }

    pub fn register_dependency_token(
        &mut self,
        element_id: ElementID,
        token: Token,
    ) -> Result<(), RegisterDependencyTokenError> {
        let permissions = Permissions::MODIFY_DEPENDENT;
        match self
            .credentials
            .register(element_id, CredentialToRegister { broker_token: token, permissions })
        {
            Err(RegisterCredentialsError::AlreadyInUse) => {
                Err(RegisterDependencyTokenError::AlreadyInUse)
            }
            Err(RegisterCredentialsError::Internal) => Err(RegisterDependencyTokenError::Internal),
            Ok(_) => Ok(()),
        }
    }

    pub fn unregister_dependency_token(
        &mut self,
        element_id: ElementID,
        token: Token,
    ) -> Result<(), UnregisterDependencyTokenError> {
        let Some(credential) = self.lookup_credentials(&token) else {
            log::debug!("unregister_dependency_token: no element found matching requires_token");
            return Err(UnregisterDependencyTokenError::NotFound);
        };
        if credential.get_element() != element_id {
            log::debug!(
                "unregister_dependency_token: token is registered to {:?}, not {:?}",
                credential.get_element(),
                element_id,
            );
            return Err(UnregisterDependencyTokenError::NotAuthorized);
        }
        self.credentials.unregister(&credential);
        Ok(())
    }

    fn current_level_satisfies(&self, required: &ElementLevel) -> bool {
        self.get_current_level(&required.element_id)
            // If current level is unknown, required is not satisfied.
            .is_some_and(|current| {
                // When the element is transitioning between states, we can't assume that the
                // current level is still a valid comparison point. Only consider the level
                // satisfied if both the current and transition levels satisfy the required level.
                let current_level_satisfies = current.satisfies(required.level);
                if let Some(transit_level) = self.in_transition.get(&required.element_id) {
                    let transit_level_satisfies = transit_level.satisfies(required.level);
                    current_level_satisfies && transit_level_satisfies
                } else {
                    current_level_satisfies
                }
            })
    }

    // Deactivate claims that are now broken due to a disorderly level transition.
    // TODO(b/356400605): Consider simplifying this function by adding support for reverse
    // dependency traversal, so that we do not have to scan all leases, but only elements
    // that directly depend on the disorderly element.
    fn deactivate_broken_claims(&mut self, element_id: ElementID, prev_level: IndexedPowerLevel) {
        let element_level = ElementLevel { element_id: element_id, level: prev_level.clone() };
        // For each lease, find the dependencies that are parents of the broken element.
        let mut affected_elements = HashSet::new();
        let mut affected_leases = HashSet::new();
        let dependencies_safe =
            self.catalog.topology.all_direct_and_indirect_dependencies(&element_level);
        self.catalog.leases.iter()
            .for_each(|(lease_id, lease)| {
                log::debug!("deactivate_broken_claims({lease_id}, {element_id}@{prev_level})");
                let lease_element_level = ElementLevel {
                    element_id: lease.synthetic_element_id,
                    level: IndexedPowerLevel { level: LeasePowerLevel::Satisfied as u8, index: 1 },
                };
                let dependencies_lease =
                    self.catalog.topology.all_direct_and_indirect_dependencies(&lease_element_level);
                if !dependencies_lease
                    .iter()
                    .any(|d| {
                        d.requires.element_id == element_id && d.requires.level.satisfies(prev_level)
                    }) {
                    log::debug!("There was no dependency in this lease that required this element to be at that level.");
                    return;
                }
                // Find the set difference between the dependencies of this lease and
                // the dependencies of the disorderly element. The element's dependencies
                // are 'safe' as none of their dependencies are broken. The difference
                // constitutes the dependencies of this lease that broke.
                let broken_dependencies =
                    HashSet::<Dependency>::from_iter(dependencies_lease)
                        .difference(&HashSet::<Dependency>::from_iter(dependencies_safe.clone()))
                        .cloned()
                        .collect::<HashSet<Dependency>>();

                for dependency in broken_dependencies {
                    let mut claim_on_broken_element = false;
                    if dependency.requires.element_id == element_id {
                        claim_on_broken_element = true;
                    }
                    let affected_claims: Vec<ClaimID> = self.catalog
                        .claims
                        .activated
                        .for_lease(*lease_id)
                        .filter(|c| c.dependency == dependency)
                        .map(|c| c.id)
                        .collect();

                    for _ in &affected_claims {
                        affected_elements.insert(dependency.requires.element_id);
                        affected_leases.insert(*lease_id);
                    }

                    if !claim_on_broken_element {
                        for id in affected_claims {
                            self.catalog.claims.deactivate_claim(id);
                        }
                    }
                }
            });
        affected_elements.remove(&element_id);
        let should_update_this_element = affected_elements.len() > 1;
        self.update_required_levels(affected_elements.into_iter(), &mut EagerInspectWriter);
        if should_update_this_element {
            self.update_required_level(element_id, prev_level.clone(), &mut EagerInspectWriter);
        }
        for lease in affected_leases {
            self.update_lease_status(lease);
        }
    }

    pub fn update_current_level(&mut self, element_id: ElementID, level: IndexedPowerLevel) {
        fuchsia_trace::duration!("power-broker", "Broker::update_current_level");
        log::debug!("update_current_level({element_id}, {level:?})");
        let is_disorderly_update = self
            .required
            .get(&element_id)
            .is_some_and(|prev_required_level| !level.satisfies(prev_required_level));

        let mut inspect_writer = UpdateLevelInspectWriter::default();
        let prev_level = self.update_current_level_internal(element_id, level, &mut inspect_writer);
        inspect_writer.commit(&self.catalog.topology);

        // Check any claims that are marked to deactivate on this element to see
        // if they can be dropped or deactivated. We do this even if the level
        // has not changed, as it is possible the last time they were checked,
        // the element was transitioning.
        let claims_marked_to_deactivate: Vec<Claim> = self
            .catalog
            .claims
            .activated
            .marked_to_deactivate_for_element(element_id)
            .cloned()
            .collect();
        if !claims_marked_to_deactivate.is_empty() {
            let claims_with_no_dependents =
                self.find_claims_to_drop_or_deactivate(&claims_marked_to_deactivate);
            self.drop_or_deactivate_claims(&claims_with_no_dependents);
        }

        if prev_level.as_ref() == Some(&level) {
            log::debug!("update_current_level({element_id}): level unchanged from {prev_level:?}");
            return;
        }
        if prev_level.is_none() || prev_level.unwrap() < level {
            // The level was increased, look for activated claims that are newly
            // satisfied by the new current level:
            log::debug!(
                "update_current_level({element_id}): level increased from {prev_level:?} to {level:?}"
            );
            // Find claims that are newly satisfied by the new level of this element:
            let claims_satisfied: Vec<Claim> = self
                .catalog
                .claims
                .activated
                .for_required_element(element_id)
                .filter(|c| {
                    level.satisfies(c.requires().level) && !prev_level.satisfies(c.requires().level)
                })
                .cloned()
                .collect();
            // Find the set of dependents for all claims satisfied:
            let dependents_of_claims_satisfied: HashSet<ElementID> =
                claims_satisfied.iter().map(|c| c.dependent().element_id).collect();
            // Because at least one of the dependencies of the dependent was
            // satisfied, other previously pending claims requiring the
            // dependent may now be ready to be activated (though they may not
            // if the dependent has other unsatisfied dependencies). Look for
            // all pending claims on this dependent, and pass them to
            // activate_claims_if_dependencies_satisfied(), which will check
            // if all dependencies of the dependent are now satisfied, and if
            // so, activate the pending claims on dependent, raising its
            // required level:
            for dependent in dependents_of_claims_satisfied {
                let pending_claims_on_dependent: Vec<Claim> =
                    self.catalog.claims.pending.for_required_element(dependent).cloned().collect();
                self.activate_claims_if_dependencies_satisfied(pending_claims_on_dependent);
            }
            // Find the set of leases for all claims satisfied:
            let leases_to_check_if_satisfied: HashSet<LeaseID> =
                claims_satisfied.iter().map(|c| c.lease_id).collect();
            // Update the status of all leases whose claims were satisfied.
            log::debug!(
                "update_current_level({element_id}): leases_to_check_if_satisfied = {:?}",
                leases_to_check_if_satisfied
            );
            for lease_id in leases_to_check_if_satisfied {
                self.update_lease_status(lease_id);
            }
            return;
        }
        if prev_level.unwrap() > level {
            log::debug!(
                "update_current_level({element_id}): level decreased from {prev_level:?} to {level:?}"
            );

            // Handle claims that were dropped unexpectedly, i.e.
            // 'disorderly' elements. When an element's level decreases
            // without a prior required level drop, we compute the set of
            // activated claims that are no longer valid and immediately
            // deactivate them.
            if is_disorderly_update {
                self.deactivate_broken_claims(element_id, prev_level.unwrap().clone());
            }
        }
    }

    pub fn get_current_level(&self, element_id: &ElementID) -> Option<IndexedPowerLevel> {
        self.current.get(element_id).map(|e| e.level)
    }

    pub fn new_current_level_subscriber(
        &mut self,
        element_id: ElementID,
    ) -> CurrentLevelSubscriber {
        self.current
            .get_mut(&element_id)
            .ok_or_else(|| anyhow!("Element ({element_id}) not added"))
            .unwrap()
            .hanging_get
            .new_subscriber()
    }

    fn update_current_level_internal<I>(
        &mut self,
        element_id: ElementID,
        level: IndexedPowerLevel,
        inspect_writer: &mut I,
    ) -> Option<IndexedPowerLevel>
    where
        I: InspectUpdateLevel,
    {
        let previous = self.get_current_level(&element_id);
        if previous == Some(level) {
            return previous;
        }
        if let Some(current_level) = self.current.get_mut(&element_id) {
            current_level.publisher.set(level);
            current_level.level = level;
        } else {
            let level_admin = LevelAdmin::<StatusWatchPowerLevelResponder>::new(level);
            self.current.insert(element_id, level_admin);
        }

        inspect_writer.update_current_level(&self.catalog.topology, element_id, level.level);

        fuchsia_trace::counter!(
            c"power-broker", c"current_level", 0,
            &self.lookup_name(element_id).into_owned() => level.level as u32
        );

        if let Some(transit_level) = self.in_transition.get(&element_id) {
            if *transit_level == level {
                log::debug!(
                    "update_current_level_internal({element_id}): transitioned to {level:?}"
                );
                self.in_transition.remove(&element_id);
                self.update_required_levels([element_id].into_iter(), inspect_writer);
            }
        }
        previous
    }

    pub fn get_required_level(&self, element_id: &ElementID) -> Option<IndexedPowerLevel> {
        self.required.get(element_id)
    }

    pub fn watch_required_level(
        &mut self,
        element_id: &ElementID,
    ) -> UnboundedReceiver<Option<IndexedPowerLevel>> {
        self.required.subscribe(element_id)
    }

    fn update_required_level<I>(
        &mut self,
        element_id: ElementID,
        level: IndexedPowerLevel,
        inspect_writer: &mut I,
    ) -> Option<IndexedPowerLevel>
    where
        I: InspectUpdateLevel,
    {
        let previous = self.get_required_level(&element_id);
        if previous == Some(level) {
            return previous;
        }
        if self.in_transition.contains_key(&element_id) {
            return previous;
        }
        self.required.update(&element_id, level);
        if let Some(current_level) = self.current.get(&element_id) {
            if current_level.level != level {
                log::debug!("update_required_level({element_id}): transitioning to {level}");
                self.in_transition.insert(element_id, level);
            }
        }
        inspect_writer.update_required_level(&self.catalog.topology, element_id, level.level);
        fuchsia_trace::counter!(
            c"power-broker", c"required_level", 0,
            &self.lookup_name(element_id).into_owned() => level.level as u32
        );

        previous
    }

    fn adjust_lease_counter(&mut self, element_id: ElementID, level: PowerLevel, adj: i64) -> i64 {
        if let Some(inner_map) = self.lease_counter.get_mut(&element_id) {
            if let Some(counter) = inner_map.get_mut(&level) {
                *counter += adj;
                return *counter;
            } else {
                inner_map.insert(level, adj);
                return adj;
            }
        } else {
            let mut inner_map = HashMap::new();
            inner_map.insert(level, adj);
            self.lease_counter.insert(element_id, inner_map);
            return adj;
        }
    }

    pub fn acquire_lease(
        &mut self,
        element_id: ElementID,
        level: IndexedPowerLevel,
        lease_control: zx::Koid,
    ) -> Result<Lease, fpb::LeaseError> {
        fuchsia_trace::duration!("power-broker", "Broker::acquire_lease");
        log::debug!("acquire_lease({element_id}@{level})");
        let counter = self.adjust_lease_counter(element_id, level.level, 1) as i64;
        fuchsia_trace::counter!(
            c"power-broker", c"LeaseCounter", level.level.into(),
            &self.lookup_name(element_id).into_owned() => counter
        );

        let lease_element_id = self.catalog.create_synthetic_lease_element(
            format!("{}_LEASE", element_id).as_str(),
            vec![ElementLevel { element_id: element_id, level: level.clone() }],
        );
        let (lease, claims) = self.catalog.create_lease_and_claims(
            lease_element_id,
            element_id,
            level,
            lease_control,
        );
        // Activate all pending claims that have all of their
        // dependencies satisfied.
        self.activate_claims_if_dependencies_satisfied(claims);
        self.update_lease_status(lease.id);
        Ok(lease)
    }

    pub fn acquire_direct_lease(
        &mut self,
        lease_name: String,
        dependencies: Vec<fpb::LeaseDependency>,
        lease_control: zx::Koid,
    ) -> Result<Lease, fpb::LeaseError> {
        fuchsia_trace::duration!("power-broker", "Broker::acquire_direct_lease");
        log::debug!("acquire_direct_lease({lease_name})");
        let mut required_levels = Vec::new();
        for dependency in dependencies {
            let Some(requires_token) = dependency.requires_token else {
                log::error!("acquire_direct_lease: dependency is missing requires_token");
                return Err(fpb::LeaseError::NotAuthorized);
            };
            let requires_token = Token::from(requires_token);
            let Some(requires_cred) = self.credentials.lookup(&requires_token) else {
                log::error!("acquire_direct_lease: unable to find required credentials");
                return Err(fpb::LeaseError::NotAuthorized);
            };
            let requires_element_id = requires_cred.get_element();
            let requires_level = if let Some(levels) = dependency.requires_level_by_preference {
                levels
                    .iter()
                    .find_map(|l| self.catalog.topology.get_level_index(requires_element_id, l))
                    .ok_or(fpb::LeaseError::InvalidLevel)?
            } else if let Some(level) = dependency.requires_level {
                self.catalog
                    .topology
                    .get_level_index(requires_element_id, &level)
                    .ok_or(fpb::LeaseError::InvalidLevel)?
            } else {
                return Err(fpb::LeaseError::InvalidLevel);
            };
            required_levels.push(ElementLevel {
                element_id: requires_element_id.clone(),
                level: requires_level.clone(),
            });
        }

        let lease_element_id = self.catalog.create_synthetic_lease_element(
            format!("{}_LEASE", lease_name).as_str(),
            required_levels,
        );

        let (lease, claims) = self.catalog.create_lease_and_claims(
            lease_element_id,
            lease_element_id,
            IndexedPowerLevel { level: LeasePowerLevel::Satisfied as u8, index: 1 },
            lease_control,
        );

        // Activate all pending claims that have all of their
        // dependencies satisfied.
        self.activate_claims_if_dependencies_satisfied(claims);
        self.update_lease_status(lease.id);
        Ok(lease)
    }

    pub fn drop_lease(&mut self, lease_id: LeaseID) -> Result<(), Error> {
        fuchsia_trace::duration!("power-broker", "Broker::drop_lease");
        // Drop the lease to mark all the relevant claims as dropped and
        // transition to PoweringDown.
        let (lease, claims) = self.catalog.drop_and_mark_powering_down(lease_id)?;
        let counter = self.adjust_lease_counter(
            lease.underlying_element_id,
            lease.underlying_element_level.level,
            -1,
        ) as i64;
        fuchsia_trace::counter!(
            c"power-broker", c"LeaseCounter", lease.underlying_element_level.level.into(),
            &self.lookup_name(lease.underlying_element_id).into_owned() => counter
        );

        // Transition the synthetic element's current level to minimum level (OFF/0) to initiate the
        // power down.
        let minimum_level = self.catalog.minimum_level(lease.synthetic_element_id);
        self.update_current_level(lease.synthetic_element_id, minimum_level);

        // Find the set of claims that can be safely dropped immediately.
        let claims_dropped = self.find_claims_to_drop_or_deactivate(&claims);
        // Drop the discovered set of claims and update required levels.
        self.drop_or_deactivate_claims(&claims_dropped);

        // Check if the lease has no remaining claims and can be vacated immediately.
        self.vacate_lease_if_all_claims_dropped(lease_id);
        Ok(())
    }

    fn vacate_lease_if_all_claims_dropped(&mut self, lease_id: LeaseID) {
        if self.catalog.is_lease_powering_down(lease_id)
            && self.catalog.has_no_remaining_claims(lease_id)
        {
            if let Err(err) = self.vacate_lease(lease_id) {
                log::error!("Failed to vacate lease {lease_id}: {:?}", err);
            }
        }
    }

    fn vacate_lease(&mut self, lease_id: LeaseID) -> Result<(), Error> {
        log::debug!("vacate_lease({lease_id})");
        let lease = self
            .catalog
            .leases
            .get(&lease_id)
            .cloned()
            .ok_or_else(|| anyhow!("{lease_id} not found"))?;

        self.catalog.lease_status.update(&lease_id, LeaseStatus::Vacated);
        if let Some(element) = self.catalog.topology.get_element(&lease.underlying_element_id) {
            self.catalog.topology.inspect().on_update_lease_status(
                &element,
                &lease,
                &LeaseStatus::Vacated,
            );
            self.catalog.topology.inspect().on_drop_lease(&element, &lease);
        }

        self.catalog.leases.remove(&lease_id);
        self.catalog.lease_status.remove(&lease_id);
        self.remove_element(&lease.synthetic_element_id);
        Ok(())
    }

    fn calculate_lease_status(&self, lease_id: LeaseID) -> LeaseStatus {
        // If the lease has any Pending claims, it is still Pending.
        if self.catalog.claims.pending.for_lease(lease_id).next().is_some() {
            return LeaseStatus::Pending;
        }

        // If the lease has any claims that have not been satisfied
        // it is still Pending.
        for claim in self.catalog.claims.activated.for_lease(lease_id) {
            if !self.current_level_satisfies(claim.requires()) {
                return LeaseStatus::Pending;
            }
        }
        // All claims are satisfied, so the lease is Satisfied.
        LeaseStatus::Satisfied
    }

    /// Re-evaluates the lease status and updates the status map.
    /// Returns the status if the overall status has changed.
    /// Returns None if no change occurred or if the lease has already been dropped.
    pub fn update_lease_status(&mut self, lease_id: LeaseID) -> Option<LeaseStatus> {
        // Return immediately if the lease has already dropped.
        if self.catalog.is_lease_dropped(lease_id) {
            return None;
        }
        // Calculate the current and previous status state.
        let status = self.calculate_lease_status(lease_id);
        let prev_status = self.catalog.lease_status.update(&lease_id, status);
        // Return if no state change has occurred.
        if prev_status.as_ref() == Some(&status) {
            return None;
        };
        log::debug!("update_lease_status({lease_id}) to {status:?}");
        // The lease_status changed, update the required level of the leased element.
        let (synthetic_element_id, underlying_element_id) = match self.catalog.leases.get(&lease_id)
        {
            Some(lease) => (lease.synthetic_element_id, lease.underlying_element_id),
            None => unreachable!("The lease must be present when updating the status."),
        };
        self.update_required_levels([synthetic_element_id].into_iter(), &mut EagerInspectWriter);
        if prev_status.as_ref() != Some(&status) {
            if let Some(element) = self.catalog.topology.get_element(&underlying_element_id) {
                let Some(lease) = self.catalog.leases.get(&lease_id) else {
                    unreachable!("The lease must be present when updating the status.");
                };
                self.catalog.topology.inspect().on_update_lease_status(&element, &lease, &status);
            }
        }
        Some(status)
    }

    #[cfg(test)]
    pub fn get_lease_status(&self, lease_id: LeaseID) -> Option<LeaseStatus> {
        self.catalog.get_lease_status(&lease_id)
    }

    pub fn watch_lease_status(
        &mut self,
        lease_id: LeaseID,
    ) -> UnboundedReceiver<Option<LeaseStatus>> {
        self.catalog.watch_lease_status(&lease_id)
    }

    fn update_required_levels<I>(
        &mut self,
        element_ids: impl IntoIterator<Item = ElementID>,
        inspect_writer: &mut I,
    ) where
        I: InspectUpdateLevel,
    {
        for element_id in element_ids {
            let new_required_level = self.catalog.calculate_required_level(element_id);
            log::debug!("update required level({:?}, {:?})", element_id, new_required_level);
            self.update_required_level(element_id, new_required_level, inspect_writer);
        }
    }

    /// Examines a Vec of pending claims and activates each for which
    /// either the required element is already at the required level (and thus
    /// the claim is already satisfied) or all of the dependencies of its required
    /// ElementLevel are met. Updates required levels of affected elements.
    /// For example, let us imagine elements A, B, C and D where A depends on B
    /// and B depends on C and D. In order to activate the A->B claim, all
    /// dependencies of B (i.e. B->C and B->D) must first be satisfied.
    fn activate_claims_if_dependencies_satisfied(
        &mut self,
        pending_claims: impl IntoIterator<Item = Claim>,
    ) {
        let claims_to_activate = pending_claims.into_iter().filter(|c| {
            // If the required element is already at the required level,
            // then the claim can immediately be activated (and is
            // already satisfied).
            self.current_level_satisfies(c.requires())
                // Otherwise, it can only be activated if all of its
                // dependencies are satisfied.
                || self.all_dependencies_satisfied(c.requires())
        });
        let (claim_ids, element_ids): (Vec<ClaimID>, Vec<ElementID>) =
            claims_to_activate.map(|c| (c.id, c.requires().element_id)).unzip();
        for claim_id in claim_ids {
            self.catalog.claims.activate_claim(claim_id);
        }

        self.update_required_levels(element_ids.into_iter(), &mut EagerInspectWriter);
    }

    /// Examines the direct dependencies of an element level
    /// and returns true if they are all satisfied (current level >= required).
    fn all_dependencies_satisfied(&self, element_level: &ElementLevel) -> bool {
        self.catalog.topology.all_direct_and_indirect_dependencies(&element_level).into_iter().all(
            |dep| {
                if !self.current_level_satisfies(&dep.requires) {
                    log::debug!(
                        "dependency {dep:?} of element_level {element_level:?} is not satisfied: \
                    current level of {:?} = {:?}, {:?} required",
                        dep.requires.element_id,
                        self.get_current_level(&dep.requires.element_id),
                        dep.requires.level
                    );
                    return false;
                }
                true
            },
        )
    }

    /// Examines a slice of claims and returns any that no longer have any
    /// other claims within their lease that require their dependent.
    fn find_claims_to_drop_or_deactivate(&mut self, claims: &[Claim]) -> Vec<Claim> {
        log::debug!("find_claims_to_drop_or_deactivate: [{}]", claims.iter().join("; "));
        let mut claims_to_drop_or_deactivate = Vec::new();

        for claim_to_check in claims {
            // If the dependent element is transiting, we cannot drop or deactivate this claim as
            // we cannot guarantee that it hasn't yet dropped to its destination level.
            if self.in_transition.contains_key(&claim_to_check.dependent().element_id) {
                log::debug!("keeping {claim_to_check}, dependent is transiting");
                continue;
            }
            // If this is an activated claim and there exists another activated claim
            // belonging to another lease that has not been dropped and whose
            // required level satisfies its required level, we can drop this claim immediately.
            if self.catalog.claims.activated.claims.contains_key(&claim_to_check.id) {
                let mut related_claims = self
                    .catalog
                    .claims
                    .activated
                    .for_required_element(claim_to_check.requires().element_id)
                    .filter(|c| c.lease_id != claim_to_check.lease_id)
                    .filter(|c| !self.catalog.is_lease_dropped(c.lease_id));
                let related_claim = related_claims.find(|related_claim| {
                    related_claim.dependent().satisfies(claim_to_check.dependent())
                        && related_claim.requires().satisfies(claim_to_check.requires())
                });
                if let Some(related_claim) = related_claim {
                    log::debug!(
                        "required level still required by another lease's activated claim({related_claim}), will drop/deactivate {claim_to_check}"
                    );
                    claims_to_drop_or_deactivate.push(claim_to_check.clone());
                    continue;
                }
            }
            if self.current_level_satisfies(claim_to_check.dependent()) {
                log::debug!("keeping {claim_to_check}, dependent is still satisfied");
                continue;
            }
            let mut has_dependents = false;
            // Only claims belonging to the same lease can be a dependent.
            for related_claim in self.catalog.claims.activated.for_lease(claim_to_check.lease_id) {
                if claim_to_check.dependent().element_id == related_claim.requires().element_id
                    && claim_to_check.dependent().level >= related_claim.requires().level
                    && self.current_level_satisfies(related_claim.requires())
                {
                    log::debug!(
                        "won't drop/deactivate {claim_to_check}, has dependent {related_claim}"
                    );
                    has_dependents = true;
                    break;
                }
            }
            if has_dependents {
                continue;
            }
            log::debug!("will drop/deactivate {claim_to_check}");
            claims_to_drop_or_deactivate.push(claim_to_check.clone());
        }
        claims_to_drop_or_deactivate
    }

    /// Takes a slice of claims, deactivates them if their lease is open,
    /// or drops them if their lease has been dropped. Then updates lease
    /// status of leases affected and required levels of elements affected.
    fn drop_or_deactivate_claims(&mut self, claims: &[Claim]) {
        let mut leases_to_check = HashSet::new();
        for claim in claims {
            log::debug!("deactivate claim: {claim}");
            if self.catalog.is_lease_dropped(claim.lease_id) {
                self.catalog.claims.drop_claim(claim.id);
                leases_to_check.insert(claim.lease_id);
            } else {
                self.catalog.claims.deactivate_claim(claim.id);
            }
        }
        self.update_required_levels(
            element_ids_required_by_claims(claims.iter()),
            &mut EagerInspectWriter,
        );
        for lease_id in leases_to_check {
            self.vacate_lease_if_all_claims_dropped(lease_id);
        }
    }

    pub fn add_element(
        &mut self,
        name: &str,
        initial_current_level: fpb::PowerLevel,
        valid_levels: Vec<fpb::PowerLevel>,
        level_dependencies: Vec<fpb::LevelDependency>,
    ) -> Result<ElementID, AddElementError> {
        fuchsia_trace::duration!("power-broker", "Broker::add_element");
        if valid_levels.len() < 1 {
            return Err(AddElementError::Invalid);
        }
        let id = self.catalog.topology.add_element(name, &valid_levels)?;
        let initial_current_level = *self
            .catalog
            .topology
            .get_level_index(id, &initial_current_level)
            .ok_or(AddElementError::Invalid)?;

        let mut inspect_writer = AddElementInspectWriter::new(id);
        self.update_current_level_internal(id, initial_current_level, &mut inspect_writer);
        let minimum_level = self.catalog.topology.minimum_level(id);
        self.update_required_level(id, minimum_level, &mut inspect_writer);

        for dependency in level_dependencies {
            if let Err(err) = self.add_dependency(id, dependency, &mut inspect_writer) {
                // Clean up by removing the element we just added.
                inspect_writer.commit(&mut self.catalog.topology);
                self.remove_element(&id);
                return Err(match err {
                    ModifyDependencyError::AlreadyExists => AddElementError::Invalid,
                    ModifyDependencyError::Invalid => AddElementError::Invalid,
                    ModifyDependencyError::NotFound(_) => AddElementError::Invalid,
                    ModifyDependencyError::NotAuthorized => AddElementError::NotAuthorized,
                });
            }
        }
        inspect_writer.commit(&mut self.catalog.topology);
        Ok(id)
    }

    #[cfg(test)]
    fn element_exists(&self, element_id: ElementID) -> bool {
        self.catalog.topology.element_exists(element_id)
    }

    pub fn remove_element(&mut self, element_id: &ElementID) {
        fuchsia_trace::duration!("power-broker", "Broker::remove_element");
        log::debug!("removing element {element_id}");
        // Before removing the element, clear any transiting state and simulate the
        // downward transition from its transiting level to its minimum level. This
        // ensures that all associated claims are cleared.
        let minimum_level = self.catalog.minimum_level(*element_id);
        if let Some(transition_level) = self.in_transition.get(&element_id) {
            if let Some(current_level) = self.current.get_mut(&element_id) {
                current_level.level = max(*transition_level, current_level.level);
            }
        }
        self.in_transition.remove(&element_id);

        // Remove all removable dependencies that require this element.
        let removable_deps =
            self.catalog.topology.get_removable_dependencies_for_required_element(*element_id);
        for dep in removable_deps {
            self.remove_dependency_and_update_leases(&dep);
        }

        let affected_leases = self.catalog.find_leases_affected_by_removal(*element_id);

        self.update_current_level(*element_id, minimum_level);
        // Remove all references of the element from the topology.
        let maybe_element = self.catalog.topology.remove_element(*element_id);

        // Cleanup orphaned claims now that the element and its dependencies are gone.
        let (elements_to_update, affected_leases) =
            self.catalog.cleanup_orphaned_claims_for_leases(affected_leases);
        self.update_required_levels(elements_to_update.into_iter(), &mut EagerInspectWriter);
        for lease_id in affected_leases {
            self.update_lease_status(lease_id);
        }

        self.unregister_all_credentials_for_element(*element_id);
        let current_level = self.current.remove(&element_id);
        let required_level = self.required.remove(&element_id);
        if let Some(element) = maybe_element {
            self.catalog.topology.inspect().on_remove_element(
                element,
                current_level.map(|level| level.level.level),
                required_level.map(|level| level.level),
            );
        }
        self.lease_counter.remove(&element_id);
    }

    fn remove_dependency_and_update_leases(&mut self, dep: &Dependency) {
        if let Ok(()) = self.catalog.topology.remove_dependency(dep) {
            let mut claims_to_drop: Vec<Claim> = Vec::new();
            for claim in self.catalog.claims.pending.claims.values() {
                if &claim.dependency == dep {
                    claims_to_drop.push(claim.clone());
                }
            }
            for claim in self.catalog.claims.activated.claims.values() {
                if &claim.dependency == dep {
                    claims_to_drop.push(claim.clone());
                }
            }
            for claim in &claims_to_drop {
                self.catalog.claims.drop_claim(claim.id);
            }
            self.update_required_levels(
                [dep.requires.element_id].into_iter(),
                &mut EagerInspectWriter,
            );
            let affected_leases: HashSet<LeaseID> =
                claims_to_drop.iter().map(|c| c.lease_id).collect();
            for lease_id in affected_leases {
                self.update_lease_status(lease_id);
            }
        }
    }

    pub fn get_level_index(
        &self,
        element_id: ElementID,
        level: &fpb::PowerLevel,
    ) -> Option<&IndexedPowerLevel> {
        self.catalog.topology.get_level_index(element_id, level)
    }

    /// Checks authorization and looks up the required element ID and level for a dependency.
    fn lookup_dependency_requires(
        &self,
        dependency: &fpb::LevelDependency,
    ) -> Result<(ElementID, IndexedPowerLevel), ModifyDependencyError> {
        let requires_token =
            dependency.requires_token.as_ref().ok_or(ModifyDependencyError::Invalid)?;
        let Some(requires_cred) = self.lookup_credentials(&Token::from(requires_token)) else {
            return Err(ModifyDependencyError::NotAuthorized);
        };
        if !requires_cred.contains(Permissions::MODIFY_DEPENDENT) {
            return Err(ModifyDependencyError::NotAuthorized);
        }
        let requires_element_id = requires_cred.get_element();

        let requires_level = dependency
            .requires_level_by_preference
            .as_ref()
            .ok_or(ModifyDependencyError::Invalid)?
            .iter()
            .find_map(|l| self.catalog.topology.get_level_index(requires_element_id.clone(), l))
            .ok_or(ModifyDependencyError::Invalid)?
            .clone();

        Ok((requires_element_id, requires_level))
    }

    /// Prepares to add a dependency by acquiring a provisional lease if needed.
    /// Returns the Lease if a provisional lease was acquired.
    pub fn prepare_add_dependency(
        &mut self,
        element_id: ElementID,
        dependency: &fpb::LevelDependency,
    ) -> Result<Option<Lease>, ModifyDependencyError> {
        let (requires_element_id, requires_level) = self.lookup_dependency_requires(dependency)?;

        let current_required = self.get_required_level(&element_id);
        let dependent_level =
            dependency.dependent_level.as_ref().ok_or(ModifyDependencyError::Invalid)?;
        let dep_level_idx = self.get_level_index(element_id, dependent_level);

        let need_provisional_lease =
            dep_level_idx.map(|dep| current_required.satisfies(dep.clone())).unwrap_or(false);

        let provisional_lease = if need_provisional_lease {
            let lease_token = zx::Event::create();
            let lease_koid = lease_token.koid().unwrap();
            let lease = self
                .acquire_lease(requires_element_id.clone(), requires_level, lease_koid)
                .unwrap();
            Some(lease)
        } else {
            None
        };

        Ok(provisional_lease)
    }

    /// Checks authorization from requires_token, and if valid, adds a dependency to the Topology.
    pub fn add_dependency<I>(
        &mut self,
        element_id: ElementID,
        dependency: fpb::LevelDependency,
        inspect_writer: &mut I,
    ) -> Result<(), ModifyDependencyError>
    where
        I: InspectAddDependency,
    {
        let (requires_element_id, requires_level) = self.lookup_dependency_requires(&dependency)?;
        let dependent_level_val =
            dependency.dependent_level.as_ref().ok_or(ModifyDependencyError::Invalid)?;
        let dependent_level = self
            .catalog
            .topology
            .get_level_index(element_id, dependent_level_val)
            .ok_or(ModifyDependencyError::Invalid)?;
        let dep = Dependency {
            dependent: ElementLevel { element_id: element_id, level: *dependent_level },
            requires: ElementLevel {
                element_id: requires_element_id.clone(),
                level: requires_level,
            },
        };
        let on_required_element_removal =
            OnRequiredElementRemoval::from_level_dependency(dependency);
        self.catalog.topology.add_dependency(&dep, on_required_element_removal, inspect_writer)?;
        self.update_leases_for_dependency(dep);
        Ok(())
    }

    pub fn update_leases_for_dependency(&mut self, dependency: Dependency) {
        let dependent_level = dependency.dependent.level;
        let dependent_id = dependency.dependent.element_id;

        let leases_to_update: HashSet<LeaseID> = self
            .catalog
            .claims
            .pending
            .for_required_element(dependent_id)
            .chain(self.catalog.claims.activated.for_required_element(dependent_id))
            .filter(|claim| claim.requires().level >= dependent_level)
            .map(|claim| claim.lease_id)
            .collect();

        let mut new_dependencies =
            self.catalog.topology.all_direct_and_indirect_dependencies(&dependency.requires);
        new_dependencies.push(dependency);

        let mut all_new_claims = Vec::new();
        for lease_id in &leases_to_update {
            let mut claims_created = Vec::new();
            for dep in &new_dependencies {
                let exists =
                    self.catalog.claims.pending.for_lease(*lease_id).any(|c| &c.dependency == dep)
                        || self
                            .catalog
                            .claims
                            .activated
                            .for_lease(*lease_id)
                            .any(|c| &c.dependency == dep);

                if !exists {
                    let claim = self.catalog.add_claim(dep.clone(), *lease_id);
                    claims_created.push(claim);
                }
            }

            let essential_claims = self.catalog.filter_out_redundant_claims(claims_created);
            for claim in essential_claims {
                self.catalog.claims.pending.add(claim.clone());
                all_new_claims.push(claim);
            }
        }

        self.activate_claims_if_dependencies_satisfied(all_new_claims);

        for lease_id in leases_to_update {
            self.update_lease_status(lease_id);
        }
    }
}

#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct LeaseID(u64);

impl fmt::Display for LeaseID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::ops::Deref for LeaseID {
    type Target = u64;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialOrd, PartialEq)]
pub struct Lease {
    pub id: LeaseID,
    // The ElementID of the synthetic element used to represent this lease.
    pub synthetic_element_id: ElementID,
    // The ElementID of the element this lease actually targets.
    pub underlying_element_id: ElementID,
    pub level: IndexedPowerLevel,
    pub underlying_element_level: IndexedPowerLevel,
}

impl Lease {
    fn new(
        synthetic_element_id: ElementID,
        underlying_element_id: ElementID,
        level: IndexedPowerLevel,
        underlying_element_level: IndexedPowerLevel,
        lease_control: zx::Koid,
    ) -> Self {
        Lease {
            id: LeaseID(lease_control.raw_koid()),
            synthetic_element_id,
            underlying_element_id,
            level: level.clone(),
            underlying_element_level: underlying_element_level.clone(),
        }
    }
}

#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct ClaimID(u64);

impl fmt::Display for ClaimID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ops::Deref for ClaimID {
    type Target = u64;
    fn deref(&self) -> &u64 {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialOrd, PartialEq)]
struct Claim {
    pub id: ClaimID,
    dependency: Dependency,
    pub lease_id: LeaseID,
}

impl fmt::Display for Claim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Claim{{{}:{:.6}: {}}}", self.lease_id, self.id, self.dependency)
    }
}

impl Claim {
    fn dependent(&self) -> &ElementLevel {
        &self.dependency.dependent
    }

    fn requires(&self) -> &ElementLevel {
        &self.dependency.requires
    }
}

/// Returns an iterator of unique ElementIDs required by claims.
fn element_ids_required_by_claims<'a, I>(claims: I) -> impl Iterator<Item = ElementID> + use<'a, I>
where
    I: IntoIterator<Item = &'a Claim>,
{
    claims.into_iter().map(|c| c.requires().element_id).unique()
}

/// Returns the maximum level required by claims, or None if empty.
fn max_level_required_by_claims<'a>(
    claims: impl IntoIterator<Item = &'a Claim>,
) -> Option<IndexedPowerLevel> {
    claims.into_iter().map(|x| x.requires().level).max()
}

#[derive(Debug)]
struct Catalog {
    topology: Topology,
    leases: HashMap<LeaseID, Lease>,
    lease_status: SubscribeMap<LeaseID, LeaseStatus>,
    /// Claims can be either Pending or Activated.
    /// Pending claims do not yet affect the required levels of their
    /// required elements. Some dependencies of their required element are not
    /// satisfied.
    /// Activated claims affect the required level of the claim's
    /// required element.
    /// Each claim will start as Pending, and will be Activated once all
    /// dependencies of its required element are satisfied.
    claims: ClaimActivationTracker,
    last_claim_id: ClaimID,
}

impl Catalog {
    fn new(inspect_parent: &INode) -> Self {
        Catalog {
            topology: Topology::new(inspect_parent, INSPECT_GRAPH_EVENT_BUFFER_SIZE),
            leases: HashMap::new(),
            lease_status: SubscribeMap::new(Some(inspect_parent.create_child("leases"))),
            claims: ClaimActivationTracker::new(),
            last_claim_id: ClaimID(0),
        }
    }

    fn next_claim_id(&mut self) -> ClaimID {
        self.last_claim_id = ClaimID(self.last_claim_id.0 + 1);
        self.last_claim_id
    }

    fn add_claim(&mut self, dependency: Dependency, lease_id: LeaseID) -> Claim {
        Claim { id: self.next_claim_id(), dependency, lease_id: lease_id }
    }

    fn find_leases_affected_by_removal(&self, removed_element_id: ElementID) -> HashSet<LeaseID> {
        let mut affected_leases = HashSet::new();

        // 1. Leases with claims requiring removed_element_id
        for claim in self.claims.pending.for_required_element(removed_element_id) {
            affected_leases.insert(claim.lease_id);
        }
        for claim in self.claims.activated.for_required_element(removed_element_id) {
            affected_leases.insert(claim.lease_id);
        }

        // 2. Leases with claims where removed_element_id is the dependent.
        // We find these by looking at what removed_element_id depended on.
        if let Some(element) = self.topology.get_element(&removed_element_id) {
            for level in &element.valid_levels {
                let el = ElementLevel { element_id: removed_element_id, level: level.clone() };
                for dep in self.topology.direct_dependencies(&el) {
                    // dep is removed_element_id -> Y
                    // We look for claims requiring Y where dependent is removed_element_id
                    let req_id = dep.requires.element_id;
                    for claim in self.claims.pending.for_required_element(req_id) {
                        if claim.dependent().element_id == removed_element_id {
                            affected_leases.insert(claim.lease_id);
                        }
                    }
                    for claim in self.claims.activated.for_required_element(req_id) {
                        if claim.dependent().element_id == removed_element_id {
                            affected_leases.insert(claim.lease_id);
                        }
                    }
                }
            }
        }
        affected_leases
    }

    /// Called after an element is removed, to remove any claims that are no longer valid from the
    /// given leases.
    fn cleanup_orphaned_claims_for_leases(
        &mut self,
        lease_ids: HashSet<LeaseID>,
    ) -> (HashSet<ElementID>, HashSet<LeaseID>) {
        let mut elements_to_update = HashSet::new();
        let mut affected_leases = HashSet::new();

        for lease_id in lease_ids {
            let Some(lease) = self.leases.get(&lease_id).cloned() else {
                continue;
            };
            let lease_element_level =
                ElementLevel { element_id: lease.synthetic_element_id, level: lease.level };
            // 1. Recalculate the valid dependencies for the lease.
            let valid_deps: HashSet<Dependency> = self
                .topology
                .all_direct_and_indirect_dependencies(&lease_element_level)
                .into_iter()
                .collect();

            // 2. Remove any orphaned pending claims for this lease. Removing a pending claim
            // can affect the lease's status, but does not affect required power levels.
            let pending_claims: Vec<Claim> =
                self.claims.pending.for_lease(lease_id).cloned().collect();
            for claim in pending_claims {
                if !valid_deps.contains(&claim.dependency) {
                    self.claims.pending.remove(claim.id);
                    affected_leases.insert(lease_id);
                }
            }

            // 3. Remove any orphaned activated claims for this lease. Removing an activated claim
            // affects both the required level of the required element and the lease's status.
            let activated_claims: Vec<Claim> =
                self.claims.activated.for_lease(lease_id).cloned().collect();
            for claim in activated_claims {
                if !valid_deps.contains(&claim.dependency) {
                    self.claims.activated.remove(claim.id);
                    elements_to_update.insert(claim.requires().element_id);
                    affected_leases.insert(lease_id);
                }
            }
        }
        (elements_to_update, affected_leases)
    }

    fn minimum_level(&self, element_id: ElementID) -> IndexedPowerLevel {
        self.topology.minimum_level(element_id)
    }

    /// Returns true if the lease was dropped (or never existed).
    fn is_lease_dropped(&self, lease_id: LeaseID) -> bool {
        if !self.leases.contains_key(&lease_id) {
            return true;
        }
        match self.lease_status.get(&lease_id) {
            Some(LeaseStatus::PoweringDown) | Some(LeaseStatus::Vacated) => true,
            _ => false,
        }
    }

    fn is_lease_powering_down(&self, lease_id: LeaseID) -> bool {
        self.lease_status.get(&lease_id) == Some(LeaseStatus::PoweringDown)
    }

    fn has_no_remaining_claims(&self, lease_id: LeaseID) -> bool {
        self.claims.pending.for_lease(lease_id).next().is_none()
            && self.claims.activated.for_lease(lease_id).next().is_none()
    }

    /// Calculates the required level for each element, according to the
    /// Minimum Power Level Policy.
    /// The required level is equal to the maximum of all **activated**
    /// claims on the element, the maximum level of all satisfied
    /// leases on the element, or the element's minimum level if there are
    /// no activated claims or satisfied leases.
    fn calculate_required_level(&self, element_id: ElementID) -> IndexedPowerLevel {
        let minimum_level = self.minimum_level(element_id);
        let activated_claims = self.claims.activated.for_required_element(element_id);
        max(
            max_level_required_by_claims(activated_claims).unwrap_or(minimum_level),
            self.calculate_level_required_by_leases(element_id).unwrap_or(minimum_level),
        )
    }

    /// Calculates the maximum level of all satisfied leases on the element.
    fn calculate_level_required_by_leases(
        &self,
        element_id: ElementID,
    ) -> Option<IndexedPowerLevel> {
        self.satisfied_leases_for_element(&element_id).map(|l| l.level).max()
    }

    /// Returns all satisfied leases for an element.
    fn satisfied_leases_for_element<'a>(
        &'a self,
        element_id: &'a ElementID,
    ) -> impl Iterator<Item = &Lease> + 'a {
        // TODO(336609941): Consider optimizing this.
        self.leases
            .values()
            .filter(|l| l.synthetic_element_id == *element_id)
            .filter(|l| self.get_lease_status(&l.id) == Some(LeaseStatus::Satisfied))
    }

    // Given a set of claims, filter out any redundant claims. A claim is redundant if there exists
    // another claim between the *same pair of elements* at an *equal or higher level*.
    fn filter_out_redundant_claims(&self, mut claims: Vec<Claim>) -> Vec<Claim> {
        let mut essential_claims: Vec<Claim> = Vec::new();
        let mut observed_pairs: HashMap<(ElementID, ElementID), ElementLevel> = HashMap::new();
        claims.sort_unstable_by_key(|claim| {
            (
                claim.dependent().element_id,
                claim.requires().element_id,
                usize::MAX - claim.requires().level.index,
            )
        });
        for claim in claims {
            let element_pair = (claim.dependent().element_id, claim.requires().element_id);
            #[allow(clippy::map_entry, reason = "mass allow for https://fxbug.dev/381896734")]
            if observed_pairs.contains_key(&element_pair) {
                continue;
            } else {
                observed_pairs.insert(element_pair, claim.requires().clone());
            }
            essential_claims.push(claim);
        }
        essential_claims
    }

    // Creates an element that represents the lease and adds it to the topology
    // with an active dependency on the actual element(s) the lease is on.
    fn create_synthetic_lease_element(
        &mut self,
        name: &str,
        required_levels: Vec<ElementLevel>,
    ) -> ElementID {
        let valid_levels = vec![LeasePowerLevel::Pending as u8, LeasePowerLevel::Satisfied as u8];
        let lease_element_id = self
            .topology
            .add_synthetic_element(
                format!("{}_{}", name, Uuid::new_v4().as_simple()).as_str(),
                &valid_levels,
            )
            .expect("Failed to create lease element");
        let mut inspect_writer = AddElementInspectWriter::new(lease_element_id);

        for requires in required_levels {
            self.topology
                .add_dependency(
                    &Dependency {
                        dependent: ElementLevel {
                            element_id: lease_element_id,
                            level: IndexedPowerLevel {
                                level: LeasePowerLevel::Satisfied as u8,
                                index: 1,
                            },
                        },
                        requires,
                    },
                    OnRequiredElementRemoval::MakeUnsatisfiable,
                    &mut inspect_writer,
                )
                .expect("Failed to attach dependency to lease element");
        }
        inspect_writer.commit(&mut self.topology);
        lease_element_id
    }

    /// Creates a new lease for the given element and level along with all
    /// claims necessary to satisfy this lease and adds them to pending_claims.
    /// Returns the new lease and the Vec of (pending) claims created.
    fn create_lease_and_claims(
        &mut self,
        lease_element_id: ElementID,
        underlying_element_id: ElementID,
        underlying_level: IndexedPowerLevel,
        lease_control: zx::Koid,
    ) -> (Lease, Vec<Claim>) {
        log::debug!(
            "create_lease_and_claims({lease_element_id} on {underlying_element_id}@{underlying_level})"
        );

        let lease = Lease::new(
            lease_element_id,
            underlying_element_id,
            IndexedPowerLevel { level: LeasePowerLevel::Satisfied as u8, index: 1 },
            underlying_level.clone(),
            lease_control,
        );
        if let Some(element) = self.topology.get_element(&underlying_element_id) {
            self.topology.inspect().on_create_lease_and_claims(
                element,
                lease.id,
                underlying_level.level,
            );
        }
        self.leases.insert(lease.id, lease.clone());

        let lease_element_level = ElementLevel {
            element_id: lease_element_id,
            level: IndexedPowerLevel { level: LeasePowerLevel::Satisfied as u8, index: 1 },
        };
        // Create all possible claims from the dependencies.
        let claims = self
            .topology
            .all_direct_and_indirect_dependencies(&lease_element_level)
            .into_iter()
            .map(|dependency| self.add_claim(dependency, lease.id))
            .collect::<Vec<Claim>>();
        // Filter claims down to only the essential (i.e. non-redundant) claims.
        let essential_claims = self.filter_out_redundant_claims(claims);
        for claim in &essential_claims {
            self.claims.pending.add(claim.clone());
        }
        (lease, essential_claims)
    }

    /// Drops an existing lease, and initiates process of releasing all
    /// associated claims, and transitions status to PoweringDown.
    /// Returns the lease and a Vec of claims marked to deactivate.
    fn drop_and_mark_powering_down(
        &mut self,
        lease_id: LeaseID,
    ) -> Result<(Lease, Vec<Claim>), Error> {
        log::debug!("drop_and_mark_powering_down(lease:{lease_id})");
        let lease =
            self.leases.get(&lease_id).cloned().ok_or_else(|| anyhow!("{lease_id} not found"))?;
        self.lease_status.update(&lease_id, LeaseStatus::PoweringDown);
        if let Some(element) = self.topology.get_element(&lease.underlying_element_id) {
            self.topology.inspect().on_update_lease_status(
                &element,
                &lease,
                &LeaseStatus::PoweringDown,
            );
        }
        // Pending claims should be dropped immediately.
        let pending_claims: Vec<ClaimID> =
            self.claims.pending.for_lease(lease.id).map(|c| c.id).collect::<Vec<_>>();
        for claim_id in pending_claims {
            if let Some(removed) = self.claims.pending.remove(claim_id) {
                log::debug!("removing pending claim: {:?}", removed);
            } else {
                log::error!("cannot remove pending claim: not found: {}", claim_id);
            }
        }
        // Claims should be marked to deactivate in an orderly sequence.
        log::debug!("drop(lease:{lease_id}): marking activated claims to deactivate");
        let claims_to_deactivate: Vec<Claim> =
            self.claims.activated.mark_to_deactivate(lease.id).collect();
        Ok((lease, claims_to_deactivate))
    }

    pub fn get_lease_status(&self, lease_id: &LeaseID) -> Option<LeaseStatus> {
        self.lease_status.get(lease_id)
    }

    fn watch_lease_status(&mut self, lease_id: &LeaseID) -> UnboundedReceiver<Option<LeaseStatus>> {
        self.lease_status.subscribe(lease_id)
    }
}

#[derive(Debug, Clone, Copy)]
enum ClaimStatus {
    Pending,
    Activated,
}

/// ClaimActivationTracker divides a set of claims into Pending and Activated
/// states, each of which can separately be accessed as a ClaimLookup.
/// Pending claims have not yet taken effect because of some prerequisite.
/// Activated claims are in effect.
/// For more details on how Pending and Activated are used, see the docs on
/// Catalog above.
#[derive(Debug)]
struct ClaimActivationTracker {
    pending: ClaimLookup,
    activated: ClaimLookup,
}

impl fmt::Display for ClaimActivationTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pending: [{}], activated: [{}]",
            self.pending.claims.values().join(", "),
            self.activated.claims.values().join(", ")
        )
    }
}

impl ClaimActivationTracker {
    fn new() -> Self {
        Self {
            pending: ClaimLookup::new(ClaimStatus::Pending),
            activated: ClaimLookup::new(ClaimStatus::Activated),
        }
    }

    /// Activates a pending claim, moving it to activated.
    fn activate_claim(&mut self, claim_id: ClaimID) {
        log::debug!("activate_claim: {claim_id}");
        self.pending.move_to(claim_id, &mut self.activated);
    }

    /// Deactivates an activated claim, moving it to pending.
    fn deactivate_claim(&mut self, claim_id: ClaimID) {
        log::debug!("deactivate_claim: {claim_id}");
        self.activated.move_to(claim_id, &mut self.pending);
        self.activated.remove_from_claims_to_deactivate(claim_id);
    }

    /// Removes a claim from both pending and activated.
    fn drop_claim(&mut self, claim_id: ClaimID) {
        log::debug!("drop_claim: {claim_id}");
        self.pending.remove(claim_id);
        self.activated.remove(claim_id);
    }
}

#[derive(Debug)]
struct ClaimLookup {
    label: &'static CStr,
    claims: HashMap<ClaimID, Claim>,
    claims_by_required_element_id: HashMap<ElementID, Vec<ClaimID>>,
    claims_by_lease: HashMap<LeaseID, Vec<ClaimID>>,
    claims_to_deactivate_by_element_id: HashMap<ElementID, Vec<ClaimID>>,
}

impl ClaimLookup {
    fn new(status: ClaimStatus) -> Self {
        let label = match status {
            ClaimStatus::Pending => c"claims_pending",
            ClaimStatus::Activated => c"claims_activated",
        };
        fuchsia_trace::counter!(
            c"power-broker", label, 0,
            "claims" => 0 as u32
        );
        Self {
            label,
            claims: HashMap::new(),
            claims_by_required_element_id: HashMap::new(),
            claims_by_lease: HashMap::new(),
            claims_to_deactivate_by_element_id: HashMap::new(),
        }
    }

    fn add(&mut self, claim: Claim) {
        self.claims_by_required_element_id
            .entry(claim.requires().element_id)
            .or_insert(Vec::new())
            .push(claim.id);
        self.claims_by_lease.entry(claim.lease_id).or_insert(Vec::new()).push(claim.id);
        self.claims.insert(claim.id, claim);
        fuchsia_trace::counter!(
            c"power-broker", self.label, 0,
            "claims" => self.claims.len() as u32
        );
    }

    fn remove(&mut self, id: ClaimID) -> Option<Claim> {
        self.remove_from_claims_to_deactivate(id);
        let Some(claim) = self.claims.remove(&id) else {
            return None;
        };
        if let Some(claim_ids) =
            self.claims_by_required_element_id.get_mut(&claim.requires().element_id)
        {
            claim_ids.retain(|x| *x != id);
            if claim_ids.is_empty() {
                self.claims_by_required_element_id.remove(&claim.requires().element_id);
            }
        }
        if let Some(claim_ids) = self.claims_by_lease.get_mut(&claim.lease_id) {
            claim_ids.retain(|x| *x != id);
            if claim_ids.is_empty() {
                self.claims_by_lease.remove(&claim.lease_id);
            }
        }
        fuchsia_trace::counter!(
            c"power-broker", self.label, 0,
            "claims" => self.claims.len() as u32
        );
        Some(claim)
    }

    fn remove_from_claims_to_deactivate(&mut self, id: ClaimID) {
        let Some(claim) = self.claims.get(&id) else {
            return;
        };
        log::debug!("remove_from_claims_to_deactivate: {claim}");
        if let Some(claim_ids) =
            self.claims_to_deactivate_by_element_id.get_mut(&claim.dependent().element_id)
        {
            claim_ids.retain(|x| *x != id);
            if claim_ids.is_empty() {
                self.claims_to_deactivate_by_element_id.remove(&claim.dependent().element_id);
            }
        }
    }
    /// Marks all claims associated with a lease to deactivate.
    /// They will be deactivated in an orderly sequence (each claim will be
    /// deactivated only once all claims dependent on it have already been
    /// deactivated).
    /// Returns an iterator of Claims marked to drop.
    fn mark_to_deactivate(&mut self, lease_id: LeaseID) -> impl Iterator<Item = Claim> {
        let claims_marked: Vec<Claim> = self.for_lease(lease_id).cloned().collect();
        log::debug!(
            "marking claims to deactivate for lease {lease_id}: [{}]",
            claims_marked.iter().join(", ")
        );
        for claim in &claims_marked {
            self.claims_to_deactivate_by_element_id
                .entry(claim.dependent().element_id)
                .or_insert(Vec::new())
                .push(claim.id);
        }
        claims_marked.into_iter()
    }

    /// Removes claim from this lookup, and adds it to recipient.
    fn move_to(&mut self, id: ClaimID, recipient: &mut ClaimLookup) {
        if let Some(claim) = self.remove(id) {
            recipient.add(claim);
        }
    }

    fn for_claim_ids<'a>(
        &'a self,
        claim_ids: &'a [ClaimID],
    ) -> impl Iterator<Item = &'a Claim> + 'a {
        claim_ids.iter().filter_map(|id| self.claims.get(id))
    }

    fn for_required_element<'a>(
        &'a self,
        element_id: ElementID,
    ) -> impl Iterator<Item = &'a Claim> + 'a {
        self.claims_by_required_element_id
            .get(&element_id)
            .into_iter()
            .flat_map(move |claim_ids| self.for_claim_ids(claim_ids))
    }

    fn for_lease<'a>(&'a self, lease_id: LeaseID) -> impl Iterator<Item = &'a Claim> + 'a {
        self.claims_by_lease
            .get(&lease_id)
            .into_iter()
            .flat_map(move |claim_ids| self.for_claim_ids(claim_ids))
    }

    /// Claims with element_id as a dependent that belong to leases which have
    /// been dropped. See ClaimLookup::mark_to_deactivate for more details.
    fn marked_to_deactivate_for_element<'a>(
        &'a self,
        element_id: ElementID,
    ) -> impl Iterator<Item = &'a Claim> + 'a {
        self.claims_to_deactivate_by_element_id
            .get(&element_id)
            .into_iter()
            .flat_map(move |claim_ids| self.for_claim_ids(claim_ids))
    }
}

trait Inspectable {
    type Value;
    fn track_inspect_with(&self, value: Self::Value, parent: &INode) -> Box<dyn IType>;
}

impl Inspectable for &ElementID {
    type Value = IndexedPowerLevel;
    fn track_inspect_with(&self, value: Self::Value, parent: &INode) -> Box<dyn IType> {
        Box::new(parent.create_uint(self.to_string(), value.level.into()))
    }
}

impl Inspectable for &LeaseID {
    type Value = LeaseStatus;
    fn track_inspect_with(&self, value: Self::Value, parent: &INode) -> Box<dyn IType> {
        Box::new(parent.create_string(format!("{self}"), format!("{:?}", value)))
    }
}

#[derive(Debug)]
struct Data<V: Clone + PartialEq> {
    value: Option<V>,
    senders: Vec<UnboundedSender<Option<V>>>,
    _inspect: Option<Box<dyn IType>>,
}

impl<V: Clone + PartialEq> Default for Data<V> {
    fn default() -> Self {
        Data { value: None, senders: Vec::new(), _inspect: None }
    }
}

/// SubscribeMap is a wrapper around a HashMap that stores values V by key K
/// and allows subscribers to register a channel on which they will receive
/// updates whenever the value stored changes.
#[derive(Debug)]
struct SubscribeMap<K: Clone + Hash + Eq, V: Clone + PartialEq> {
    values: HashMap<K, Data<V>>,
    inspect: Option<INode>,
}

impl<K: Clone + Hash + Eq, V: Clone + PartialEq> SubscribeMap<K, V> {
    fn new(inspect: Option<INode>) -> Self {
        SubscribeMap { values: HashMap::new(), inspect }
    }

    fn get(&self, key: &K) -> Option<V> {
        self.values.get(key).map(|d| d.value.clone()).flatten()
    }

    // update updates the value for key.
    // Returns previous value, if any.
    fn update<'a>(&mut self, key: &'a K, value: V) -> Option<V>
    where
        &'a K: Inspectable<Value = V>,
        V: Copy,
    {
        let previous = self.get(key);
        // If the value hasn't changed, this is a no-op, return.
        if previous.as_ref() == Some(&value) {
            return previous;
        }
        let mut senders = Vec::new();
        if let Some(Data { value: _, senders: old_senders, _inspect: _ }) = self.values.remove(&key)
        {
            // Prune invalid senders.
            for sender in old_senders {
                if let Err(err) = sender.unbounded_send(Some(value.clone())) {
                    if err.is_disconnected() {
                        continue;
                    }
                }
                senders.push(sender);
            }
        }
        let _inspect = self.inspect.as_mut().map(|inspect| key.track_inspect_with(value, &inspect));
        let value = Some(value);
        self.values.insert(key.clone(), Data { value, senders, _inspect });
        previous
    }

    fn subscribe(&mut self, key: &K) -> UnboundedReceiver<Option<V>> {
        let (sender, receiver) = unbounded::<Option<V>>();
        sender.unbounded_send(self.get(key)).expect("initial send should not fail");
        self.values.entry(key.clone()).or_default().senders.push(sender);
        receiver
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        self.values.remove(key).and_then(|data| data.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{AnyProperty, PropertyAssertion, assert_data_tree};
    use fidl_fuchsia_power_broker::{BinaryPowerLevel, DependencyToken};
    use fuchsia_inspect::hierarchy::DiagnosticsHierarchy;

    use power_broker_client::BINARY_POWER_LEVELS;

    // Convenience aliases.
    const OFF: IndexedPowerLevel =
        IndexedPowerLevel { level: BinaryPowerLevel::Off.into_primitive(), index: 0 };
    const ON: IndexedPowerLevel =
        IndexedPowerLevel { level: BinaryPowerLevel::On.into_primitive(), index: 1 };

    const ZERO: IndexedPowerLevel = IndexedPowerLevel::from_same_level_and_index(0);
    const ONE: IndexedPowerLevel = IndexedPowerLevel::from_same_level_and_index(1);
    const TWO: IndexedPowerLevel = IndexedPowerLevel::from_same_level_and_index(2);

    #[track_caller]
    fn assert_element_cleaned_up(broker: &Broker, element_id: ElementID) {
        assert!(
            !broker.catalog.topology.element_exists(element_id),
            "topology.elements not cleaned up"
        );
        assert_eq!(broker.in_transition.get(&element_id), None, "in_transition not cleaned up");
        assert!(!broker.current.contains_key(&element_id), "current not cleaned up");
        assert_eq!(broker.required.get(&element_id), None, "required not cleaned up");
        assert_eq!(broker.lease_counter.get(&element_id), None, "lease_counter not cleaned up");
    }

    #[track_caller]
    fn assert_lease_cleaned_up(catalog: &Catalog, lease_id: LeaseID) {
        assert!(!catalog.leases.contains_key(&lease_id), "{lease_id} still in catalog.leases");
        assert!(
            catalog.lease_status.get(&lease_id).is_none(),
            "{lease_id} still in catalog.lease_status"
        );
        assert_eq!(
            catalog.claims.activated.for_lease(lease_id).count(),
            0,
            "claims.activated not empty"
        );
        assert_eq!(
            catalog.claims.pending.for_lease(lease_id).count(),
            0,
            "claims.pending not empty"
        );
    }

    #[track_caller]
    fn assert_events_recorded_in_order_helper(
        hierarchy: &DiagnosticsHierarchy,
        expected_events: &[(&'static str, Vec<(&'static str, Box<dyn PropertyAssertion>)>)],
    ) {
        let events_node = hierarchy
            .get_child_by_path(&["test", "topology", "events"])
            .expect("events node found");

        let mut expected_idx = 0;

        for child_node in events_node.children.iter().rev() {
            if expected_idx >= expected_events.len() {
                break;
            }
            let (expected_name, expected_properties) = &expected_events[expected_idx];
            if let Some(event_node) = child_node.get_child(expected_name) {
                let matches = expected_properties.iter().all(|(prop_name, assertion)| {
                    if let Some(property) = event_node.get_property(prop_name) {
                        assertion.run(property).is_ok()
                    } else {
                        false
                    }
                });
                if matches {
                    expected_idx += 1;
                }
            }
        }

        if expected_idx != expected_events.len() {
            println!("--- INSPECT EVENTS RECORDED ---");
            for child_node in events_node.children.iter().rev() {
                println!("Event: {:?}", child_node);
            }
            println!("---------------------------------");
        }

        assert_eq!(
            expected_idx,
            expected_events.len(),
            "Failed to find all expected events in chronological order. Found {}/{} expected events.",
            expected_idx,
            expected_events.len()
        );
    }

    macro_rules! assert_events_recorded_in_order {
        ($hierarchy:expr, [ $( { $event:ident: { $( $prop:ident: $val:expr $(,)? )* } $(,)? } $(,)? )* ]) => {{
            let expected = vec![
                $(
                    (
                        stringify!($event),
                        vec![
                            $(
                                (stringify!($prop), Box::new($val) as Box<dyn PropertyAssertion>),
                            )*
                        ]
                    ),
                )*
            ];
            assert_events_recorded_in_order_helper($hierarchy, &expected);
        }};
    }

    #[fuchsia::test]
    fn test_binary_satisfy_power_level() {
        for (level, required, want) in
            [(OFF, ON, false), (OFF, OFF, true), (ON, OFF, true), (ON, ON, true)]
        {
            let got = level.satisfies(required);
            assert_eq!(
                got, want,
                "{:?}.satisfies({:?}) = {:?}, want {:?}",
                level, required, got, want
            );
        }
    }

    #[fuchsia::test]
    fn test_user_defined_satisfy_power_level() {
        for (level, required, want) in [
            (0, 1, false),
            (0, 0, true),
            (1, 0, true),
            (1, 1, true),
            (255, 0, true),
            (255, 1, true),
            (255, 255, true),
            (1, 255, false),
            (35, 36, false),
            (35, 35, true),
        ] {
            let level = IndexedPowerLevel::from_same_level_and_index(level);
            let required = IndexedPowerLevel::from_same_level_and_index(required);
            let got = level.satisfies(required);
            assert_eq!(
                got, want,
                "{:?}.satisfies({:?}) = {:?}, want {:?}",
                level, required, got, want
            );
        }
    }

    #[fuchsia::test]
    fn test_option_satisfy_power_level() {
        for (level, required, want) in [
            (None, 0, false),
            (None, 1, false),
            (Some(0), 1, false),
            (Some(0), 0, true),
            (Some(1), 0, true),
            (Some(1), 1, true),
            (Some(255), 0, true),
            (Some(255), 1, true),
            (Some(255), 255, true),
            (Some(1), 255, false),
            (Some(35), 36, false),
            (Some(35), 35, true),
        ] {
            let level = level.map(|l| IndexedPowerLevel::from_same_level_and_index(l));
            let required = IndexedPowerLevel::from_same_level_and_index(required);
            let got = level.satisfies(required);
            assert_eq!(
                got, want,
                "{:?}.satisfies({:?}) = {:?}, want {:?}",
                level, required, got, want
            );
        }
    }

    #[fuchsia::test]
    fn test_levels() {
        let mut levels = SubscribeMap::<ElementID, IndexedPowerLevel>::new(None);

        let element_a = ElementID::new(1);
        let element_b = ElementID::new(2);
        levels.update(&element_a, ON);
        assert_eq!(levels.get(&element_a), Some(ON));
        assert_eq!(levels.get(&element_b), None);

        levels.update(&element_a, OFF);
        levels.update(&element_b, ON);
        assert_eq!(levels.get(&element_a), Some(OFF));
        assert_eq!(levels.get(&element_b), Some(ON));

        let element_ud1 = ElementID::new(3);
        let element_ud2 = ElementID::new(4);
        levels.update(&element_ud1, IndexedPowerLevel::from_same_level_and_index(145));
        assert_eq!(
            levels.get(&element_ud1),
            Some(IndexedPowerLevel::from_same_level_and_index(145))
        );
        assert_eq!(levels.get(&element_ud2), None);

        levels.update(&element_a, ON);
        levels.remove(&element_b);
        assert_eq!(levels.get(&element_b), None);
    }

    #[fuchsia::test]
    fn test_levels_subscribe() {
        let mut levels = SubscribeMap::<ElementID, IndexedPowerLevel>::new(None);

        let element_a = ElementID::new(1);
        let element_b = ElementID::new(2);
        let mut receiver_a = levels.subscribe(&element_a);
        let mut receiver_b = levels.subscribe(&element_b);

        levels.update(&element_a, ON);
        assert_eq!(levels.get(&element_a), Some(ON));
        assert_eq!(levels.get(&element_b), None);

        levels.update(&element_a, OFF);
        levels.update(&element_b, ON);
        assert_eq!(levels.get(&element_a), Some(OFF));
        assert_eq!(levels.get(&element_b), Some(ON));

        let mut received_a = Vec::new();
        while let Ok(Some(level)) = receiver_a.try_next() {
            received_a.push(level)
        }
        assert_eq!(received_a, vec![None, Some(ON), Some(OFF)]);
        let mut received_b = Vec::new();
        while let Ok(Some(level)) = receiver_b.try_next() {
            received_b.push(level)
        }
        assert_eq!(received_b, vec![None, Some(ON)]);
    }

    fn create_test_claim(
        id: u64,
        dependent_element_id: ElementID,
        dependent_element_level: fpb::PowerLevel,
        requires_element_id: ElementID,
        requires_element_level: fpb::PowerLevel,
    ) -> Claim {
        Claim {
            id: ClaimID(id),
            dependency: Dependency {
                dependent: ElementLevel {
                    element_id: dependent_element_id,
                    level: IndexedPowerLevel::from_same_level_and_index(dependent_element_level),
                },
                requires: ElementLevel {
                    element_id: requires_element_id,
                    level: IndexedPowerLevel::from_same_level_and_index(requires_element_level),
                },
            },
            lease_id: LeaseID(0),
        }
    }

    #[fuchsia::test]
    fn test_claim_lookup_add_remove() {
        let mut lookup = ClaimLookup::new(ClaimStatus::Activated);

        let element_a = ElementID::new(1);
        let element_b = ElementID::new(2);
        let claim_a_1_b_1 = create_test_claim(1, element_a, 1, element_b, 1);
        let claim_a_2_b_2 = create_test_claim(2, element_a, 2, element_b, 2);

        lookup.add(claim_a_1_b_1.clone());
        lookup.add(claim_a_2_b_2.clone());

        assert_eq!(
            lookup.mark_to_deactivate(claim_a_2_b_2.lease_id).collect::<Vec<_>>(),
            vec![claim_a_1_b_1.clone(), claim_a_2_b_2.clone()]
        );

        assert_eq!(lookup.remove(claim_a_1_b_1.id), Some(claim_a_1_b_1.clone()));
        assert_eq!(lookup.remove(claim_a_2_b_2.id), Some(claim_a_2_b_2.clone()));
        assert_eq!(lookup.remove(claim_a_2_b_2.id), None);

        assert_eq!(lookup.claims.len(), 0);
        assert_eq!(lookup.claims_by_required_element_id.len(), 0);
        assert_eq!(lookup.claims_by_lease.len(), 0);
        assert_eq!(lookup.claims_to_deactivate_by_element_id.len(), 0);
    }

    #[fuchsia::test]
    fn test_filter_out_redundant_claims() {
        let inspect = fuchsia_inspect::Inspector::default();
        let broker = Broker::new(inspect.root().create_child("test"));

        let element_a = ElementID::new(1);
        let element_b = ElementID::new(2);
        let element_c = ElementID::new(3);
        let claim_a_1_b_1 = create_test_claim(1, element_a, 1, element_b, 1);
        let claim_a_2_b_2 = create_test_claim(2, element_a, 2, element_b, 2);
        let claim_a_1_c_1 = create_test_claim(3, element_a, 1, element_c, 1);
        let claim_b_1_c_1 = create_test_claim(4, element_b, 1, element_c, 1);
        let claim_a_2_c_2 = create_test_claim(5, element_a, 2, element_c, 2);

        //  A     B
        //  1 ==> 1 (redundant with A@2=>B@2)
        //  2 ==> 2
        let essential_claims = broker
            .catalog
            .filter_out_redundant_claims(vec![claim_a_1_b_1.clone(), claim_a_2_b_2.clone()]);
        assert_eq!(essential_claims, vec![claim_a_2_b_2.clone()]);

        //  A     B     C
        //  1 ========> 1 (not redundant, not between same elements)
        //  2 ==> 2
        let essential_claims = broker
            .catalog
            .filter_out_redundant_claims(vec![claim_a_1_c_1.clone(), claim_a_2_b_2.clone()]);
        assert_eq!(essential_claims, vec![claim_a_2_b_2.clone(), claim_a_1_c_1.clone()]);

        //  A     B     C
        //  1 ==> 1 ==> 1 (not redundant, A@2=>C@2 cannot satisfy B@1=>C@1, not between same elements)
        //  2 ========> 2
        let essential_claims = broker.catalog.filter_out_redundant_claims(vec![
            claim_a_1_b_1.clone(),
            claim_b_1_c_1.clone(),
            claim_a_2_c_2.clone(),
        ]);
        assert_eq!(
            essential_claims,
            vec![claim_a_1_b_1.clone(), claim_a_2_c_2.clone(), claim_b_1_c_1.clone()]
        );
    }

    #[fuchsia::test]
    async fn test_initialize_current_and_broker_status() {
        let inspect = fuchsia_inspect::Inspector::default();
        let inspect_node = inspect.root().create_child("test");
        let mut broker = Broker::new(inspect_node);
        let latinum = broker
            .add_element(
                "Latinum",
                7,
                // Unsorted. The order declares the order of increasing power.
                vec![5, 2, 7],
                vec![],
            )
            .expect("add_element failed");
        assert_eq!(broker.lookup_name(latinum), "Latinum".to_string());
        assert_eq!(
            broker.get_current_level(&latinum),
            Some(IndexedPowerLevel { level: 7, index: 2 })
        );
        assert_eq!(
            broker.get_required_level(&latinum),
            Some(IndexedPowerLevel { level: 5, index: 0 })
        );

        assert_data_tree!(inspect, root: {
            test: {
                leases: {},
                topology: {
                    events: {
                        "0": {
                            "@time": AnyProperty,
                            add_element: {
                                element_id: *broker.get_unsatisfiable_element_id(),
                                current_level: "unset",
                                required_level: "unset",
                            }
                        },
                        "1": {
                            "@time": AnyProperty,
                            add_element: {
                                element_id: *latinum,
                                current_level: 7u64,
                                required_level: 5u64,
                            }
                        },
                    },
                    stats: contains {},
                    "fuchsia.inspect.Graph": {
                        topology: {
                            broker.get_unsatisfiable_element_id().to_string() => {
                                meta: {
                                    name: broker.get_unsatisfiable_element_name(),
                                    valid_levels: broker.get_unsatisfiable_element_levels(),
                                    required_level: "unset",
                                    current_level: "unset",
                                    leases: {}
                                },
                                relationships: {}
                            },
                            latinum.to_string() => {
                                meta: {
                                    name: "Latinum",
                                    valid_levels: vec![5u64, 2u64, 7u64],
                                    current_level: 7u64,
                                    required_level: 5u64,
                                    leases: {}
                                },
                                relationships: {},
                            },
                        },
        }}}});
    }

    #[fuchsia::test]
    async fn test_current_required_level_inspect() {
        let inspect = fuchsia_inspect::Inspector::default();
        let inspect_node = inspect.root().create_child("test");
        let mut broker = Broker::new(inspect_node);
        let latinum =
            broker.add_element("Latinum", 2, vec![0, 1, 2], vec![]).expect("add_element failed");
        assert_eq!(broker.get_current_level(&latinum), Some(TWO));
        assert_eq!(broker.get_required_level(&latinum), Some(ZERO));

        // Update current level to 0 to preserve ordering.
        broker.update_current_level(latinum, ZERO);

        // Update required level to 1.
        broker.update_required_level(latinum, ONE, &mut EagerInspectWriter);

        // Update required level to 1 again, should have no additional effect.
        broker.update_required_level(latinum, ONE, &mut EagerInspectWriter);

        // Update current level to 1.
        // This should drop the current required level to ZERO, because there
        // isn't any claim preserving the required level update.
        broker.update_current_level(latinum, ONE);

        // Update current level to 1 again, should have no additional effect.
        broker.update_current_level(latinum, ONE);

        assert_data_tree!(inspect, root: {
        test: {
            leases: {},
            topology: {
                "fuchsia.inspect.Graph": {
                    topology: {
                        broker.get_unsatisfiable_element_id().to_string() => {
                            meta: {
                                name: broker.get_unsatisfiable_element_name(),
                                valid_levels: broker.get_unsatisfiable_element_levels(),
                                required_level: "unset",
                                current_level: "unset",
                                leases: {},
                            },
                            relationships: {}
                        },
                        latinum.to_string() => {
                            meta: {
                                name: "Latinum",
                                valid_levels: vec![0u64, 1u64, 2u64],
                                current_level: 1u64,
                                required_level: 0u64,
                                leases: {},
                            },
                            relationships: {},
                        },
                    },
                },
                stats: contains {},
                events: {
                    "0": {
                        "@time": AnyProperty,
                        add_element: {
                            element_id: *broker.get_unsatisfiable_element_id(),
                            current_level: "unset",
                            required_level: "unset",
                        }
                    },
                    "1": {
                        "@time": AnyProperty,
                        add_element: {
                            element_id: *latinum,
                            current_level: 2u64,
                            required_level: 0u64,
                        }
                    },
                    "2": {
                        "@time": AnyProperty,
                        update_level: {
                            element_id: *latinum,
                            current_level: 0u64,
                        }
                    },
                    "3": {
                        "@time": AnyProperty,
                        update_level: {
                            element_id: *latinum,
                            required_level: 1u64,
                        }
                    },
                    "4": {
                        "@time": AnyProperty,
                        update_level: {
                            element_id: *latinum,
                            current_level: 1u64,
                            required_level: 0u64,
                        }
                    },
                },
            }}});
    }

    #[fuchsia::test]
    async fn test_add_element_dependency_never_and_unregistered() {
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));
        let token_mithril = DependencyToken::create();
        let never_registered_token = DependencyToken::create();
        let mithril = broker
            .add_element("Mithril", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                mithril,
                token_mithril.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let v01: Vec<u64> = BINARY_POWER_LEVELS.iter().map(|&v| v as u64).collect();
        assert_data_tree!(inspect, root: {
            test: {
                leases: {},
                topology: {
                    events: contains {},
                    stats: contains {},
                    "fuchsia.inspect.Graph": {
                        topology: {
                            broker.get_unsatisfiable_element_id().to_string() => {
                                meta: {
                                    name: broker.get_unsatisfiable_element_name(),
                                    valid_levels: broker.get_unsatisfiable_element_levels(),
                                    required_level: "unset",
                                    current_level: "unset",
                                    leases: {}
                                },
                                relationships: {}
                            },
                            mithril.to_string() => {
                                meta: {
                                    name: "Mithril",
                                    valid_levels: v01.clone(),
                                    current_level: OFF.level as u64,
                                    required_level: OFF.level as u64,
                                    leases: {}
                                },
                                relationships: {},
                            },
                        },
                    },
                },
        }});

        let hierarchy = fuchsia_inspect::reader::read(&inspect).await.unwrap();
        assert_events_recorded_in_order!(
            &hierarchy,
            [
                {
                    add_element: {
                        element_id: *broker.get_unsatisfiable_element_id(),
                        current_level: "unset",
                        required_level: "unset",
                    },
                },
                {
                    add_element: {
                        element_id: *mithril,
                        current_level: 0u64,
                        required_level: 0u64,
                    },
                },
            ]
        );

        // This should fail, because the token was never registered.
        let add_element_not_authorized_res = broker.add_element(
            "Silver",
            OFF.level,
            BINARY_POWER_LEVELS.to_vec(),
            vec![fpb::LevelDependency {
                dependent_level: Some(ON.level),
                requires_token: Some(
                    never_registered_token
                        .duplicate_handle(zx::Rights::SAME_RIGHTS)
                        .expect("dup failed"),
                ),
                requires_level_by_preference: Some(vec![ON.level]),
                ..Default::default()
            }],
        );
        assert!(matches!(add_element_not_authorized_res, Err(AddElementError::NotAuthorized)));

        // Add element with a valid token should succeed.
        let silver = broker
            .add_element(
                "Silver",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        token_mithril
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        assert_data_tree!(inspect, root: {
        test: {
            leases: {},
            topology: {
                events: contains {},
                stats: contains {},
                "fuchsia.inspect.Graph": {
                    topology: {
                        broker.get_unsatisfiable_element_id().to_string() => {
                            meta: {
                                name: broker.get_unsatisfiable_element_name(),
                                valid_levels: broker.get_unsatisfiable_element_levels(),
                                required_level: "unset",
                                current_level: "unset",
                                leases: {}
                            },
                            relationships: {}
                        },
                        mithril.to_string() => {
                            meta: {
                                name: "Mithril",
                                valid_levels: v01.clone(),
                                current_level: OFF.level as u64,
                                required_level: OFF.level as u64,
                                leases: {}
                            },
                            relationships: {},
                        },
                        silver.to_string() => {
                            meta: {
                                name: "Silver",
                                valid_levels: v01.clone(),
                                current_level: OFF.level as u64,
                                required_level: OFF.level as u64,
                                leases: {}
                            },
                            relationships: {
                                mithril.to_string() => {
                                    edge_id: AnyProperty,
                                    meta: {
                                      "1": {
                                          required_level: 1u64,
                                      },
                                    },
                                },
                            },
                        },
                    },
                }
            }}});

        let hierarchy = fuchsia_inspect::reader::read(&inspect).await.unwrap();
        assert_events_recorded_in_order!(
            &hierarchy,
            [
                {
                    add_element: {
                        element_id: *broker.get_unsatisfiable_element_id(),
                        current_level: "unset",
                        required_level: "unset",
                    },
                },
                {
                    add_element: {
                        element_id: *mithril,
                        current_level: 0u64,
                        required_level: 0u64,
                    },
                },
                {
                    add_element: {
                        element_id: *silver,
                        current_level: 0u64,
                        required_level: 0u64,
                    },
                },
            ]
        );

        // Unregister token_mithril, then try to add again, which should fail.
        broker
            .unregister_dependency_token(
                mithril,
                token_mithril.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("unregister_dependency_token failed");

        let add_element_not_authorized_res: Result<ElementID, AddElementError> = broker
            .add_element(
                "Silver",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        token_mithril
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            );
        assert!(matches!(add_element_not_authorized_res, Err(AddElementError::NotAuthorized)));
    }

    #[fuchsia::test]
    async fn test_remove_element() {
        let inspect = fuchsia_inspect::Inspector::default();
        let inspect_node = inspect.root().create_child("test");
        let mut broker = Broker::new(inspect_node);
        let unobtanium = broker
            .add_element("Unobtainium", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        assert_eq!(broker.element_exists(unobtanium), true);
        let v01: Vec<u64> = BINARY_POWER_LEVELS.iter().map(|&v| v as u64).collect();
        assert_data_tree!(inspect, root: {
        test: {
            leases: {},
            topology: {
                "fuchsia.inspect.Graph": {
                    topology: {
                        broker.get_unsatisfiable_element_id().to_string() => {
                            meta: {
                                name: broker.get_unsatisfiable_element_name(),
                                valid_levels: broker.get_unsatisfiable_element_levels(),
                                required_level: "unset",
                                current_level: "unset",
                                leases: {}
                            },
                            relationships: {}
                        },
                        unobtanium.to_string() => {
                            meta: {
                                name: "Unobtainium",
                                valid_levels: v01.clone(),
                                current_level: OFF.level as u64,
                                required_level: OFF.level as u64,
                                leases: {}
                            },
                            relationships: {},
                        },
                    },
                },
                stats: contains {},
                events: {
                    "0": {
                        "@time": AnyProperty,
                        add_element: {
                            element_id: *broker.get_unsatisfiable_element_id(),
                            current_level: "unset",
                            required_level: "unset",
                        }
                    },
                    "1": {
                        "@time": AnyProperty,
                        add_element: {
                            element_id: *unobtanium,
                            current_level: OFF.level as u64,
                            required_level: OFF.level as u64,
                        }
                    },
                },
            }
        }});

        broker.remove_element(&unobtanium);
        assert_eq!(broker.element_exists(unobtanium), false);
        assert_data_tree!(inspect, root: {
        test: {
            leases: {},
            topology: {
                events: {
                    "0": {
                        "@time": AnyProperty,
                        add_element: {
                            element_id: *broker.get_unsatisfiable_element_id(),
                            current_level: "unset",
                            required_level: "unset",
                        }
                    },
                    "1": {
                        "@time": AnyProperty,
                        add_element: {
                            element_id: *unobtanium,
                            current_level: OFF.level as u64,
                            required_level: OFF.level as u64,
                        }
                    },
                    "2": {
                        "@time": AnyProperty,
                        rm_element: {
                            element_id: *unobtanium,
                            name: "Unobtainium",
                            valid_levels: v01.clone(),
                            current_level: OFF.level as u64,
                            required_level: OFF.level as u64,
                        }
                    },
                },
                stats: contains {},
                "fuchsia.inspect.Graph": {
                    topology: {
                        broker.get_unsatisfiable_element_id().to_string() => {
                            meta: {
                                name: broker.get_unsatisfiable_element_name(),
                                valid_levels: broker.get_unsatisfiable_element_levels(),
                                required_level: "unset",
                                current_level: "unset",
                                leases: {}
                            },
                            relationships: {}
                        },
                    },
                }
            }}});
    }

    struct BrokerStatusMatcher {
        lease: LeaseMatcher,
        required_level: RequiredLevelMatcher,
    }

    impl BrokerStatusMatcher {
        fn new() -> Self {
            Self { lease: LeaseMatcher::new(), required_level: RequiredLevelMatcher::new() }
        }

        #[track_caller]
        fn assert_matches(&self, broker: &Broker) {
            self.lease.assert_matches(broker);
            self.required_level.assert_matches(broker);
        }
    }

    struct RequiredLevelMatcher {
        elements: HashMap<ElementID, IndexedPowerLevel>,
    }

    impl RequiredLevelMatcher {
        fn new() -> Self {
            Self { elements: HashMap::new() }
        }

        fn update(&mut self, id: ElementID, required_level: IndexedPowerLevel) {
            self.elements.insert(id, required_level);
        }

        fn remove(&mut self, id: ElementID) {
            self.elements.remove(&id);
        }

        #[track_caller]
        fn assert_matches(&self, broker: &Broker) {
            for (id, expected) in &self.elements {
                let rl = broker.get_required_level(id).unwrap();
                assert_eq!(rl, *expected, "get_required_level({id}) = {rl}, expected = {expected}");
            }
        }
    }

    struct LeaseMatcher {
        leases: HashMap<LeaseID, LeaseStatus>,
    }

    impl LeaseMatcher {
        fn new() -> Self {
            Self { leases: HashMap::new() }
        }

        fn update(&mut self, id: LeaseID, status: LeaseStatus) {
            self.leases.insert(id, status);
        }

        fn remove(&mut self, id: LeaseID) {
            self.leases.remove(&id);
        }

        #[track_caller]
        fn assert_matches(&self, broker: &Broker) {
            for (id, expected_status) in &self.leases {
                let status = broker.get_lease_status(*id).expect(
                    "No lease exists with id ({id}), forgot to remove it from the matcher?",
                );
                assert_eq!(
                    status, *expected_status,
                    "get_lease_status({id}) = {status:?}, expected = {expected_status:?}"
                );
            }
        }
    }

    #[fuchsia::test]
    fn test_broker_adjust_lease_counter() {
        // Create a broker that has nothing since the test is just about the counter
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));

        let a = broker
            .add_element("a", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");

        let b = broker
            .add_element("b", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");

        assert_eq!(broker.adjust_lease_counter(a, 5, 1), 1);
        // a:5: 1
        assert_eq!(broker.adjust_lease_counter(a, 5, 1), 2);
        // a:5: 2
        assert_eq!(broker.adjust_lease_counter(b, 6, 1), 1);
        // a:5: 2 b:6:1
        assert_eq!(broker.adjust_lease_counter(a, 15, 1), 1);
        // a:5: 2 a:15:1 b:6:1

        assert_eq!(broker.adjust_lease_counter(a, 5, 1), 3);
        // a:5: 3 a:15:1 b:6:1
        assert_eq!(broker.adjust_lease_counter(a, 5, -1), 2);
        // a:5: 2 a:15:1 b:6:1
        assert_eq!(broker.adjust_lease_counter(b, 6, -1), 0);
        // a:5: 2 a:15:1 b:6:0
        assert_eq!(broker.adjust_lease_counter(a, 5, -1), 1);
        // a:5: 1 a:15:1 b:6:0
        assert_eq!(broker.adjust_lease_counter(a, 15, -1), 0);
        // a:5: 1 a:15:0 b:6:0
    }

    #[fuchsia::test]
    async fn test_broker_lease_direct() {
        // Create a topology of a child element with two direct dependencies.
        // P1 <= C => P2
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));
        let parent1_token = DependencyToken::create();
        let parent1: ElementID = broker
            .add_element("P1", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                parent1,
                parent1_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let parent2_token = DependencyToken::create();
        let parent2: ElementID = broker
            .add_element("P2", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                parent2,
                parent2_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let child = broker
            .add_element(
                "C",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![
                    fpb::LevelDependency {
                        dependent_level: Some(ON.level),
                        requires_token: Some(
                            parent1_token
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .expect("dup failed"),
                        ),
                        requires_level_by_preference: Some(vec![ON.level]),
                        ..Default::default()
                    },
                    fpb::LevelDependency {
                        dependent_level: Some(ON.level),
                        requires_token: Some(
                            parent2_token
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .expect("dup failed"),
                        ),
                        requires_level_by_preference: Some(vec![ON.level]),
                        ..Default::default()
                    },
                ],
            )
            .expect("add_element failed");

        let mut broker_status = BrokerStatusMatcher::new();

        // All elements should start with required level OFF.
        broker_status.required_level.update(parent1, OFF);
        broker_status.required_level.update(parent2, OFF);
        broker_status.required_level.update(child, OFF);
        broker_status.assert_matches(&broker);

        let v01: Vec<u64> = BINARY_POWER_LEVELS.iter().map(|&v| v as u64).collect();
        assert_data_tree!(inspect, root: {
            test: {
                leases: {},
                topology: {
                    events: contains {},
                    stats: contains {},
                    "fuchsia.inspect.Graph": {
                        topology: {
                            broker.get_unsatisfiable_element_id().to_string() => {
                                meta: {
                                    name: broker.get_unsatisfiable_element_name(),
                                    valid_levels: broker.get_unsatisfiable_element_levels(),
                                    required_level: "unset",
                                    current_level: "unset",
                                    leases: {}
                                },
                                relationships: {}
                            },
                            parent1.to_string() => {
                                meta: {
                                    name: "P1",
                                    valid_levels: v01.clone(),
                                    current_level: OFF.level as u64,
                                    required_level: OFF.level as u64,
                                    leases: {}
                                },
                                relationships: {},
                            },
                            parent2.to_string() => {
                                meta: {
                                    name: "P2",
                                    valid_levels: v01.clone(),
                                    current_level: OFF.level as u64,
                                    required_level: OFF.level as u64,
                                    leases: {}
                                },
                                relationships: {},
                            },
                            child.to_string() => {
                                meta: {
                                    name: "C",
                                    valid_levels: v01.clone(),
                                    current_level: OFF.level as u64,
                                    required_level: OFF.level as u64,
                                    leases: {}
                                },
                                relationships: {
                                    parent1.to_string() => {
                                        edge_id: AnyProperty,
                                        meta: {
                                            "1": {
                                                required_level: 1u64,
                                            }
                                        },
                                    },
                                    parent2.to_string() => {
                                        edge_id: AnyProperty,
                                        meta: {
                                            "1": {
                                                required_level: 1u64,
                                            }
                                        },
                                    },
                                },
                            },
                        },
                    },
        }}});

        // Acquiring the lease should result in two direct claims.
        // P1's required level should become ON.
        // P2's required level should become ON.
        // The lease should be pending, as C isn't ON yet.
        let lease = broker.acquire_lease(child, ON, zx::Koid::from_raw(1)).expect("acquire failed");
        broker_status.required_level.update(parent1, ON);
        broker_status.required_level.update(parent2, ON);
        broker_status.lease.update(lease.id, LeaseStatus::Pending);
        broker_status.assert_matches(&broker);

        assert_eq!(broker.adjust_lease_counter(child, ON.level, 0), 1);

        // Update P1's current level to ON.
        // C's required level should not change, as it also requires P2.
        broker.update_current_level(parent1, ON);
        broker_status.assert_matches(&broker);

        // Update P2's current level to ON.
        // C's required level should become ON, as both P1 and P2 are satisfied.
        broker.update_current_level(parent2, ON);
        broker_status.required_level.update(child, ON);
        broker_status.assert_matches(&broker);

        // Update C's current level to ON.
        // The lease should now be satisfied.
        broker.update_current_level(child, ON);
        broker_status.lease.update(lease.id, LeaseStatus::Satisfied);
        broker_status.assert_matches(&broker);

        // We expect one synthetic element.
        let (synthetic_id, synthetic_name) = {
            let mut synthetic_elements =
                broker.catalog.topology.elements.iter().filter(|(_, e)| e.synthetic);
            let (_, synthetic) = synthetic_elements.next().unwrap();
            assert!(synthetic_elements.next().is_none());
            (synthetic.id, synthetic.name.clone())
        };

        assert_data_tree!(inspect, root: {
            test: {
                leases: {
                    lease.id.to_string() => "Satisfied",
                },
                topology: {
                    events: contains {},
                    stats: contains {},
                    "fuchsia.inspect.Graph": {
                        topology: {
                            broker.get_unsatisfiable_element_id().to_string() => {
                                meta: {
                                    name: broker.get_unsatisfiable_element_name(),
                                    valid_levels: broker.get_unsatisfiable_element_levels(),
                                    required_level: "unset",
                                    current_level: "unset",
                                    leases: {},
                                },
                                relationships: {}
                            },
                            synthetic_id.to_string() => {
                                meta: {
                                    name: synthetic_name,
                                    synthetic: true,
                                    leases: {},
                                    valid_levels: vec![0u64, 255],
                                },
                                relationships: {
                                    child.to_string() => {
                                        edge_id: AnyProperty,
                                        meta: {
                                            "255": {
                                                required_level: ON.level as u64,
                                            }
                                        }
                                    }
                                },
                            },
                            parent1.to_string() => {
                                meta: {
                                    name: "P1",
                                    valid_levels: v01.clone(),
                                    current_level: ON.level as u64,
                                    required_level: ON.level as u64,
                                    leases: {},
                                },
                                relationships: {},
                            },
                            parent2.to_string() => {
                                meta: {
                                    name: "P2",
                                    valid_levels: v01.clone(),
                                    current_level: ON.level as u64,
                                    required_level: ON.level as u64,
                                    leases: {},
                                },
                                relationships: {},
                            },
                            child.to_string() => {
                                meta: {
                                    name: "C",
                                    valid_levels: v01.clone(),
                                    current_level: ON.level as u64,
                                    required_level: ON.level as u64,
                                    leases: {
                                        format!("{}", lease.id) => {
                                            level: 1u64,
                                            status: 2u64,
                                        }
                                    }
                                },
                                relationships: {
                                    parent1.to_string() => {
                                        edge_id: AnyProperty,
                                        meta: {
                                            "1": {
                                                required_level: 1u64,
                                            }
                                        }
                                    },
                                    parent2.to_string() => {
                                        edge_id: AnyProperty,
                                        meta: {
                                            "1": {
                                                required_level: 1u64,
                                            }
                                        }
                                    },
                                },
                            },
                        },
                    },
        }}});

        let hierarchy = fuchsia_inspect::reader::read(&inspect).await.unwrap();
        assert_events_recorded_in_order!(
            &hierarchy,
            [
                {
                    add_element: {
                        element_id: *synthetic_id,
                    },
                },
                {
                    create_lease: {
                        element_id: *child,
                        lease_id: *lease.id,
                        level: 1u64,
                    },
                },
                {
                    update_level: {
                        element_id: *synthetic_id,
                        required_level: 0u64,
                    },
                },
                {
                    update_lease: {
                        element_id: *child,
                        lease_id: *lease.id,
                        status: 1u64,
                    },
                },
                {
                    update_level: {
                        element_id: *synthetic_id,
                        required_level: 255u64,
                    },
                },
                {
                    update_lease: {
                        element_id: *child,
                        lease_id: *lease.id,
                        status: 2u64,
                    },
                },
            ]
        );

        // Dropping the lease should cause both claims to be dropped.
        // C's required level should become OFF.
        // P1 and P2's required level should remain ON, as C depends on them.
        broker.drop_lease(lease.id).expect("drop failed");
        broker_status.required_level.update(child, OFF);
        broker_status.lease.remove(lease.id);
        broker_status.assert_matches(&broker);

        assert_eq!(broker.adjust_lease_counter(child, ON.level, 0), 0);

        // Drop C's level to OFF.
        // P1's required level should become OFF, as no lease requires it.
        // P2's required level should become OFF, as no lease requires it.
        broker.update_current_level(child, OFF);
        broker_status.required_level.update(parent1, OFF);
        broker_status.required_level.update(parent2, OFF);
        broker_status.assert_matches(&broker);

        // Try dropping the lease one more time, which should result in an error.
        let extra_drop = broker.drop_lease(lease.id);
        assert!(extra_drop.is_err());

        assert_lease_cleaned_up(&broker.catalog, lease.id);

        // Because ElementIDs are randomly generated, these will be processed in order of ID hash
        // value. We don't actually care about the order for correctness outside of
        // assert_events_recordered_in_order.
        let (first_parent, second_parent) =
            if parent1 < parent2 { (parent1, parent2) } else { (parent2, parent1) };

        let hierarchy = fuchsia_inspect::reader::read(&inspect).await.unwrap();
        assert_events_recorded_in_order!(
            &hierarchy,
            [
                {
                    add_element: {
                        element_id: *synthetic_id,
                    },
                },
                {
                    create_lease: {
                        element_id: *child,
                        lease_id: *lease.id,
                        level: 1u64,
                    },
                },
                {
                    update_level: {
                        element_id: *synthetic_id,
                        required_level: 0u64,
                    },
                },
                {
                    update_lease: {
                        element_id: *child,
                        lease_id: *lease.id,
                        status: 1u64,
                    },
                },
                {
                    update_level: {
                        element_id: *synthetic_id,
                        required_level: 255u64,
                    },
                },
                {
                    update_lease: {
                        element_id: *child,
                        lease_id: *lease.id,
                        status: 2u64,
                    },
                },
                {
                    update_lease: {
                        element_id: *child,
                        lease_id: *lease.id,
                        status: 3u64,
                    },
                },
                {
                    update_level: {
                        element_id: *synthetic_id,
                        current_level: 0u64,
                    },
                },
                {
                    update_level: {
                        element_id: *child,
                        required_level: 0u64,
                    },
                },
                {
                    update_level: {
                        element_id: *child,
                        current_level: 0u64,
                    },
                },
                {
                    update_level: {
                        element_id: *first_parent,
                        required_level: 0u64,
                    },
                },
                {
                    update_level: {
                        element_id: *second_parent,
                        required_level: 0u64,
                    },
                },
                {
                    update_lease: {
                        element_id: *child,
                        lease_id: *lease.id,
                        status: 4u64,
                    },
                },
                {
                    rm_lease: {
                        element_id: *child,
                        lease_id: *lease.id,
                        level: 1u64,
                    },
                },
            ]
        );

        broker.remove_element(&child);
        assert_element_cleaned_up(&broker, child);
        broker.remove_element(&parent2);
        assert_element_cleaned_up(&broker, parent2);
        broker.remove_element(&parent1);
        assert_element_cleaned_up(&broker, parent1);
    }

    #[fuchsia::test]
    fn test_broker_lease_transitive() {
        // Create a topology of a child element with two chained transitive
        // dependencies.
        // C => P => GP
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));
        let grandparent_token = DependencyToken::create();
        let grandparent: ElementID = broker
            .add_element("GP", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                grandparent,
                grandparent_token
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("dup failed")
                    .into(),
            )
            .expect("register_dependency_token failed");
        let parent_token = DependencyToken::create();
        let parent: ElementID = broker
            .add_element("P", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                parent,
                parent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let child = broker
            .add_element(
                "C",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        parent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        let dep = fpb::LevelDependency {
            dependent_level: Some(ON.level),
            requires_token: Some(
                grandparent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
            ),
            requires_level_by_preference: Some(vec![ON.level]),
            ..Default::default()
        };
        broker.add_dependency(parent, dep, &mut EagerInspectWriter).expect("add_dependency failed");
        let mut broker_status = BrokerStatusMatcher::new();

        // All elements should start with required level OFF.
        broker_status.required_level.update(parent, OFF);
        broker_status.required_level.update(grandparent, OFF);
        broker_status.required_level.update(child, OFF);
        broker_status.assert_matches(&broker);

        // Lease C, which will result in two claims, one for P as the direct
        // dependency and GP as the transitive dependency.
        // P's required level should remain OFF, it depends on GP.
        // GP's required level should become ON.
        let lease = broker.acquire_lease(child, ON, zx::Koid::from_raw(1)).expect("acquire failed");
        broker_status.required_level.update(grandparent, ON);
        broker_status.lease.update(lease.id, LeaseStatus::Pending);
        broker_status.assert_matches(&broker);

        // Raise GP's level to ON.
        // P's required level should become ON.
        broker.update_current_level(grandparent, ON);
        broker_status.required_level.update(parent, ON);
        broker_status.assert_matches(&broker);

        // Raise P's level to ON.
        // C's required level should become ON.
        broker.update_current_level(parent, ON);
        broker_status.required_level.update(child, ON);
        broker_status.assert_matches(&broker);

        // Raise C's level to ON.
        // Lease C should now be satisfied as C is ON.
        broker.update_current_level(child, ON);
        broker_status.lease.update(lease.id, LeaseStatus::Satisfied);
        broker_status.assert_matches(&broker);

        // Drop Lease C.
        // C's required level should become OFF, as it's claim is dropped.
        // P's required level should remain ON, until C is OFF.
        // GP's required level should remain ON, until P is OFF.
        broker.drop_lease(lease.id).expect("drop failed");
        broker_status.required_level.update(child, OFF);
        broker_status.lease.remove(lease.id);
        broker_status.assert_matches(&broker);

        // Lower C's level to OFF.
        // P's required level should become OFF.
        broker.update_current_level(child, OFF);
        broker_status.required_level.update(parent, OFF);
        broker_status.assert_matches(&broker);

        // Lower P's required level to OFF.
        // GP's required level should become OFF.
        broker.update_current_level(parent, OFF);
        broker_status.required_level.update(grandparent, OFF);
        broker_status.assert_matches(&broker);

        assert_lease_cleaned_up(&broker.catalog, lease.id);
    }

    #[fuchsia::test]
    fn test_broker_transitive_dependency_removed_element() {
        // Create a topology of a child element with three chained transitive
        // dependencies: C => P => GP => GGP.
        // If P is removed while C is leased, the claims on GP and GGP should be dropped.
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));

        let greatgrandparent_token = DependencyToken::create();
        let greatgrandparent: ElementID = broker
            .add_element("GGP", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                greatgrandparent,
                greatgrandparent_token
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("dup failed")
                    .into(),
            )
            .expect("register_dependency_token failed");

        let grandparent_token = DependencyToken::create();
        let grandparent: ElementID = broker
            .add_element("GP", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                grandparent,
                grandparent_token
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("dup failed")
                    .into(),
            )
            .expect("register_dependency_token failed");

        let parent_token = DependencyToken::create();
        let parent: ElementID = broker
            .add_element("P", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                parent,
                parent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");

        let child = broker
            .add_element(
                "C",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        parent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");

        // P depends on GP
        let dep_p_gp = fpb::LevelDependency {
            dependent_level: Some(ON.level),
            requires_token: Some(
                grandparent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
            ),
            requires_level_by_preference: Some(vec![ON.level]),
            ..Default::default()
        };
        broker
            .add_dependency(parent, dep_p_gp, &mut EagerInspectWriter)
            .expect("add_dependency failed");

        // GP depends on GGP
        let dep_gp_ggp = fpb::LevelDependency {
            dependent_level: Some(ON.level),
            requires_token: Some(
                greatgrandparent_token
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("dup failed"),
            ),
            requires_level_by_preference: Some(vec![ON.level]),
            ..Default::default()
        };
        broker
            .add_dependency(grandparent, dep_gp_ggp, &mut EagerInspectWriter)
            .expect("add_dependency failed");

        let mut broker_status = BrokerStatusMatcher::new();

        // All elements should start with required level OFF.
        broker_status.required_level.update(parent, OFF);
        broker_status.required_level.update(grandparent, OFF);
        broker_status.required_level.update(greatgrandparent, OFF);
        broker_status.required_level.update(child, OFF);
        broker_status.assert_matches(&broker);

        // Lease C, which will result in GGP's required level becoming ON.
        let lease = broker.acquire_lease(child, ON, zx::Koid::from_raw(1)).expect("acquire failed");
        broker_status.required_level.update(greatgrandparent, ON);
        broker_status.lease.update(lease.id, LeaseStatus::Pending);
        broker_status.assert_matches(&broker);

        // Raise GGP's level to ON.
        broker.update_current_level(greatgrandparent, ON);
        // GP's required level should become ON.
        broker_status.required_level.update(grandparent, ON);
        broker_status.assert_matches(&broker);

        // Raise GP's level to ON.
        broker.update_current_level(grandparent, ON);
        // P's required level should become ON.
        broker_status.required_level.update(parent, ON);
        broker_status.assert_matches(&broker);

        // Raise P's level to ON.
        broker.update_current_level(parent, ON);
        // C's required level should become ON.
        broker_status.required_level.update(child, ON);
        broker_status.assert_matches(&broker);

        // Raise C's level to ON.
        broker.update_current_level(child, ON);
        // Lease should now be satisfied.
        broker_status.lease.update(lease.id, LeaseStatus::Satisfied);
        broker_status.assert_matches(&broker);

        // Remove P (the intermediate element).
        // This breaks the transitive chain C => P => GP => GGP.
        // The claims on GP and GGP should be dropped.
        broker.remove_element(&parent);

        // P is removed, so we shouldn't check it.
        broker_status.required_level.remove(parent);
        broker_status.required_level.update(child, OFF);
        broker_status.required_level.update(grandparent, OFF);
        broker_status.required_level.update(greatgrandparent, OFF);
        // Lease should become Pending again because dependency chain is broken.
        broker_status.lease.update(lease.id, LeaseStatus::Pending);
        broker_status.assert_matches(&broker);

        // Clean up remaining elements
        broker.remove_element(&child);
        assert_element_cleaned_up(&broker, child);
        broker.remove_element(&grandparent);
        assert_element_cleaned_up(&broker, grandparent);
        broker.remove_element(&greatgrandparent);
        assert_element_cleaned_up(&broker, greatgrandparent);
    }

    #[fuchsia::test]
    fn test_broker_lease_shared() {
        // Create a topology of two child elements with a shared
        // parent and grandparent
        // C1 \\
        //      > P => GP
        // C2 //
        // Child 1 requires Parent at 50 to support its own level of 5.
        // Parent requires Grandparent at 200 to support its own level of 50.
        // C1 => P => GP
        //  5 => 50 => 200
        // Child 2 requires Parent at 30 to support its own level of 3.
        // Parent requires Grandparent at 90 to support its own level of 30.
        // C2 =>  P => GP
        //  3 => 30 => 90
        // Grandparent has a minimum required level of 10.
        // All other elements have a minimum of 0.
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));
        let grandparent_token = DependencyToken::create();
        let grandparent: ElementID =
            broker.add_element("GP", 10, vec![10, 90, 200], vec![]).expect("add_element failed");
        broker
            .register_dependency_token(
                grandparent,
                grandparent_token
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("dup failed")
                    .into(),
            )
            .expect("register_dependency_token failed");
        let parent_token = DependencyToken::create();
        let parent: ElementID = broker
            .add_element(
                "P",
                0,
                vec![0, 30, 50],
                vec![
                    fpb::LevelDependency {
                        dependent_level: Some(50),
                        requires_token: Some(
                            grandparent_token
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .expect("dup failed"),
                        ),
                        requires_level_by_preference: Some(vec![200]),
                        ..Default::default()
                    },
                    fpb::LevelDependency {
                        dependent_level: Some(30),
                        requires_token: Some(
                            grandparent_token
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .expect("dup failed"),
                        ),
                        requires_level_by_preference: Some(vec![90]),
                        ..Default::default()
                    },
                ],
            )
            .expect("add_element failed");
        broker
            .register_dependency_token(
                parent,
                parent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let child1 = broker
            .add_element(
                "C1",
                0,
                vec![0, 5],
                vec![fpb::LevelDependency {
                    dependent_level: Some(5),
                    requires_token: Some(
                        parent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![50]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        let child2 = broker
            .add_element(
                "C2",
                0,
                vec![0, 3],
                vec![fpb::LevelDependency {
                    dependent_level: Some(3),
                    requires_token: Some(
                        parent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![30]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");

        // Initially, all elements should be at their default required levels.
        // Grandparent should have a default required level of 10
        // and all others should have a default required level of 0.
        let mut broker_status = BrokerStatusMatcher::new();
        broker_status.required_level.update(parent, ZERO);
        broker_status.required_level.update(grandparent, IndexedPowerLevel { level: 10, index: 0 });
        broker_status.required_level.update(child1, ZERO);
        broker_status.required_level.update(child2, ZERO);
        broker_status.assert_matches(&broker);

        // Acquire lease for Child 1. Initially, Grandparent should have
        // required level 200 and Parent should have required level 0
        // because Child 1 has a dependency on Parent and Parent has a
        // dependency on Grandparent. Grandparent has no dependencies so its
        // level should be raised first.
        let lease1 = broker
            .acquire_lease(child1, IndexedPowerLevel { level: 5, index: 1 }, zx::Koid::from_raw(1))
            .expect("acquire failed");
        broker_status
            .required_level
            .update(grandparent, IndexedPowerLevel { level: 200, index: 2 });
        broker_status.lease.update(lease1.id, LeaseStatus::Pending);
        broker_status.assert_matches(&broker);

        // Raise Grandparent's current level to 200. Now Parent claim should
        // be enforced, because its dependency on Grandparent is unblocked
        // raising its required level to 50.
        broker.update_current_level(grandparent, IndexedPowerLevel { level: 200, index: 2 });
        broker_status.required_level.update(parent, IndexedPowerLevel { level: 50, index: 2 });
        broker_status.assert_matches(&broker);

        // Update Parent's current level to 50.
        // Parent and Grandparent should have required levels of 50 and 200.
        broker.update_current_level(parent, IndexedPowerLevel { level: 50, index: 2 });
        broker_status.required_level.update(child1, IndexedPowerLevel { level: 5, index: 1 });
        broker_status.assert_matches(&broker);

        // Update Child 1's current level to 5.
        // Lease Child 1's is now satisfied.
        broker.update_current_level(child1, IndexedPowerLevel { level: 5, index: 1 });
        broker_status.lease.update(lease1.id, LeaseStatus::Satisfied);
        broker_status.assert_matches(&broker);

        // Acquire a lease for Child 2. Though Child 2 has nominal
        // requirements of Parent at 30 and Grandparent at 100, they are
        // superseded by Child 1's requirements of 50 and 200.
        let lease2 = broker
            .acquire_lease(child2, IndexedPowerLevel { level: 3, index: 1 }, zx::Koid::from_raw(2))
            .expect("acquire failed");
        broker_status.required_level.update(child2, IndexedPowerLevel { level: 3, index: 1 });
        broker_status.lease.update(lease2.id, LeaseStatus::Pending);
        broker_status.assert_matches(&broker);

        // Update Child 2's current level to 3.
        // Lease Child 2's is now satisfied.
        broker.update_current_level(child2, IndexedPowerLevel { level: 3, index: 1 });
        broker_status.lease.update(lease2.id, LeaseStatus::Satisfied);
        broker_status.assert_matches(&broker);

        // Drop lease for Child 1.
        // Child's required level should be 0.
        broker.drop_lease(lease1.id).expect("drop failed");
        broker_status.required_level.update(child1, ZERO);
        broker_status.lease.remove(lease1.id);
        broker_status.assert_matches(&broker);

        // Drop Child 1's level to 0.
        // Parent's required level should immediately drop to 30.
        // Grandparent's required level will remain at 200 for now.
        broker.update_current_level(child1, ZERO);
        broker_status.required_level.update(parent, IndexedPowerLevel { level: 30, index: 1 });
        broker_status.lease.remove(lease1.id);
        broker_status.assert_matches(&broker);

        // Lower Parent's current level to 30. Now Grandparent's required level
        // should drop to 90.
        broker.update_current_level(parent, IndexedPowerLevel { level: 30, index: 1 });
        broker_status.required_level.update(grandparent, IndexedPowerLevel { level: 90, index: 1 });
        broker_status.assert_matches(&broker);

        // All claims for Lease 1 should now be cleaned up,
        // even though Lease 2 is still active.
        assert_lease_cleaned_up(&broker.catalog, lease1.id);

        // Drop lease for Child 2.
        // Child 2's required level should drop to 0.
        broker.drop_lease(lease2.id).expect("drop failed");
        broker_status.required_level.update(child2, ZERO);
        broker_status.lease.remove(lease2.id);
        broker_status.assert_matches(&broker);

        // Lower Child 2's current level to 0.
        // Parent should have required level 0.
        // Grandparent's required level should remain 90.
        broker.update_current_level(child2, ZERO);
        broker_status.required_level.update(parent, ZERO);
        broker_status.assert_matches(&broker);

        // Lower GrandParent's current level to 90.
        broker.update_current_level(grandparent, IndexedPowerLevel { level: 90, index: 1 });
        broker_status.assert_matches(&broker);

        // Lower Parent's current level to 0. Grandparent claim should now be
        // dropped and have its default required level of 10.
        broker.update_current_level(parent, ZERO);
        broker_status.required_level.update(grandparent, IndexedPowerLevel { level: 10, index: 0 });
        broker_status.assert_matches(&broker);
        assert_lease_cleaned_up(&broker.catalog, lease2.id);
    }

    #[fuchsia::test]
    fn test_broker_lease_redundant_claims() {
        // Create a topology of two child elements with a shared
        // parent and grandparent
        // C1 \\
        //      > P => GP
        // C2 //
        // Child 1 ON requires Parent ON.
        // Parent ON requires Grandparent ON.
        // C1 => P => GP
        // Child 2 ON requires Parent ON.
        // Parent ON requires Grandparent ON.
        // C2 =>  P => GP
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));
        let grandparent_token = DependencyToken::create();
        let grandparent: ElementID = broker
            .add_element("GP", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                grandparent,
                grandparent_token
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("dup failed")
                    .into(),
            )
            .expect("register_dependency_token failed");
        let parent_token = DependencyToken::create();
        let parent: ElementID = broker
            .add_element(
                "P",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        grandparent_token
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        broker
            .register_dependency_token(
                parent,
                parent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let child1 = broker
            .add_element(
                "C1",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        parent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        let child2 = broker
            .add_element(
                "C2",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        parent_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");

        // Initially, all elements should be OFF.
        let mut broker_status = BrokerStatusMatcher::new();
        broker_status.required_level.update(parent, OFF);
        broker_status.required_level.update(grandparent, OFF);
        broker_status.required_level.update(child1, OFF);
        broker_status.required_level.update(child2, OFF);
        broker_status.assert_matches(&broker);

        // Acquire lease for Child 1. Initially, Grandparent
        // should have required level ON because Child 1 has a dependency
        // on Parent and Parent has a dependency on Grandparent. Grandparent
        // has no dependencies so its level should be raised first.
        let lease1 =
            broker.acquire_lease(child1, ON, zx::Koid::from_raw(1)).expect("acquire failed");
        broker_status.required_level.update(grandparent, ON);
        broker_status.lease.update(lease1.id, LeaseStatus::Pending);
        broker_status.assert_matches(&broker);

        // Raise Grandparent's current level to ON. Now Parent claim should
        // be enforced, because its dependency on Grandparent is unblocked
        // raising its required level to ON.
        broker.update_current_level(grandparent, ON);
        broker_status.required_level.update(parent, ON);
        broker_status.assert_matches(&broker);

        // Update Parent's current level to ON.
        // Child 1's required level should become ON.
        broker.update_current_level(parent, ON);
        broker_status.required_level.update(child1, ON);
        broker_status.assert_matches(&broker);

        // Update Child 1's current level to ON.
        // Lease Child 1's is now satisfied.
        broker.update_current_level(child1, ON);
        broker_status.lease.update(lease1.id, LeaseStatus::Satisfied);
        broker_status.assert_matches(&broker);

        // Acquire a lease for Child 2. Child 2 requires Parent and
        // Grandparent ON, which is already met by Child 1's requirements.
        let lease2 =
            broker.acquire_lease(child2, ON, zx::Koid::from_raw(2)).expect("acquire failed");
        broker_status.required_level.update(child2, ON);
        broker_status.lease.update(lease2.id, LeaseStatus::Pending);
        broker_status.assert_matches(&broker);

        // Update Child 2's current level to ON.
        // Lease Child 2's is now satisfied.
        broker.update_current_level(child2, ON);
        broker_status.lease.update(lease2.id, LeaseStatus::Satisfied);
        broker_status.assert_matches(&broker);

        // Drop lease for Child 1.
        // Child's required level should be OFF.
        broker.drop_lease(lease1.id).expect("drop failed");
        broker_status.required_level.update(child1, OFF);
        broker_status.lease.remove(lease1.id);
        broker_status.assert_matches(&broker);

        // Drop Child 1's level to OFF.
        // Parent's required level should not be affected.
        // Grandparent's required level should not be affected.
        broker.update_current_level(child1, OFF);
        broker_status.lease.remove(lease1.id);
        broker_status.assert_matches(&broker);

        // All claims for Lease 1 should now be cleaned up,
        // even though Lease 2 is still active.
        assert_lease_cleaned_up(&broker.catalog, lease1.id);

        // Drop lease for Child 2.
        // Child 2's required level should drop to OFF.
        broker.drop_lease(lease2.id).expect("drop failed");
        broker_status.required_level.update(child2, OFF);
        broker_status.lease.remove(lease2.id);
        broker_status.assert_matches(&broker);

        // Lower Child 2's current level to OFF.
        // Parent should have required level OFF.
        // Grandparent's required level should remain ON.
        broker.update_current_level(child2, OFF);
        broker_status.required_level.update(parent, OFF);
        broker_status.assert_matches(&broker);

        // Lower Parent's current level to OFF. Grandparent claim should now be
        // dropped and have its default required level of OFF.
        broker.update_current_level(parent, OFF);
        broker_status.required_level.update(grandparent, OFF);
        broker_status.assert_matches(&broker);
        assert_lease_cleaned_up(&broker.catalog, lease2.id);
    }

    #[fuchsia::test]
    fn test_broker_lease_redundant_claims_shared_downstream() {
        // Create a topology of two pairs of chained elements with
        // a shared downstream dependency
        // A => B => C
        //           C <= D <= E
        // A ON requires B ON.
        // B ON requires C ON.
        // E ON requires D ON.
        // D ON requires C ON.
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));
        let element_c_token = DependencyToken::create();
        let element_c: ElementID = broker
            .add_element("C", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                element_c,
                element_c_token
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("dup failed")
                    .into(),
            )
            .expect("register_dependency_token failed");
        let element_b_token = DependencyToken::create();
        let element_b: ElementID = broker
            .add_element(
                "B",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        element_c_token
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        broker
            .register_dependency_token(
                element_b,
                element_b_token
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("dup failed")
                    .into(),
            )
            .expect("register_dependency_token failed");
        let element_a = broker
            .add_element(
                "A",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        element_b_token
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        let element_d = broker
            .add_element(
                "D",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        element_c_token
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        let element_d_token = DependencyToken::create();
        broker
            .register_dependency_token(
                element_d,
                element_d_token
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("dup failed")
                    .into(),
            )
            .expect("register_dependency_token failed");
        let element_e = broker
            .add_element(
                "E",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        element_d_token
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");

        log::warn!("a: {element_a}\nb: {element_b}\nc:{element_c}\nd:{element_d}\ne:{element_e}");

        // Initially, all elements should be OFF.
        let mut broker_status = BrokerStatusMatcher::new();
        broker_status.required_level.update(element_a, OFF);
        broker_status.required_level.update(element_b, OFF);
        broker_status.required_level.update(element_c, OFF);
        broker_status.required_level.update(element_d, OFF);
        broker_status.required_level.update(element_e, OFF);
        broker_status.assert_matches(&broker);

        // Acquire lease for E. C's required level should become on as a result.
        let lease1 =
            broker.acquire_lease(element_e, ON, zx::Koid::from_raw(1)).expect("acquire failed");
        broker_status.required_level.update(element_c, ON);
        broker_status.lease.update(lease1.id, LeaseStatus::Pending);
        broker_status.assert_matches(&broker);

        // Raise C's current level to ON. Now D should have its required level on.
        broker.update_current_level(element_c, ON);
        broker_status.required_level.update(element_d, ON);
        broker_status.assert_matches(&broker);

        // Update D's current level to ON. Now E should have its required level on.
        broker.update_current_level(element_d, ON);
        broker_status.required_level.update(element_e, ON);
        broker_status.assert_matches(&broker);

        // Raise E's current level to ON. Now the lease should be satisfied.
        broker.update_current_level(element_e, ON);
        broker_status.lease.update(lease1.id, LeaseStatus::Satisfied);
        broker_status.assert_matches(&broker);

        // Acquire a lease for A. A requires B, which is currently OFF and
        // C, which is already ON.
        let lease2 =
            broker.acquire_lease(element_a, ON, zx::Koid::from_raw(2)).expect("acquire failed");
        broker_status.required_level.update(element_b, ON);
        broker_status.lease.update(lease2.id, LeaseStatus::Pending);
        broker_status.assert_matches(&broker);

        // Update B's current level to ON.
        // A's required level should become ON.
        broker.update_current_level(element_b, ON);
        broker_status.required_level.update(element_a, ON);
        broker_status.assert_matches(&broker);

        // Update A's current level to ON.
        // The lease on A is now satisfied.
        broker.update_current_level(element_a, ON);
        broker_status.lease.update(lease2.id, LeaseStatus::Satisfied);
        broker_status.assert_matches(&broker);

        // Drop lease for A.
        // A's required level should become OFF.
        broker.drop_lease(lease2.id).expect("drop failed");
        broker_status.required_level.update(element_a, OFF);
        broker_status.lease.remove(lease2.id);
        broker_status.assert_matches(&broker);

        // Drop lease for E.
        // E's required level should become OFF.
        broker.drop_lease(lease1.id).expect("drop failed");
        broker_status.required_level.update(element_e, OFF);
        broker_status.lease.remove(lease1.id);
        broker_status.assert_matches(&broker);

        // Lower E's level to OFF.
        // D's required level should become OFF.
        broker.update_current_level(element_e, OFF);
        broker_status.required_level.update(element_d, OFF);
        broker_status.assert_matches(&broker);

        // Lower A's level to OFF.
        // B's required level should become OFF.
        broker.update_current_level(element_a, OFF);
        broker_status.required_level.update(element_b, OFF);
        broker_status.assert_matches(&broker);

        // Lower D's level to OFF.
        // No required levels should change.
        broker.update_current_level(element_d, OFF);
        broker_status.assert_matches(&broker);

        // Lower B's level to OFF.
        // C's required level should now become OFF.
        broker.update_current_level(element_b, OFF);
        broker_status.required_level.update(element_c, OFF);
        broker_status.assert_matches(&broker);

        // Lower C's level to OFF.
        broker.update_current_level(element_b, OFF);
        broker_status.assert_matches(&broker);

        // All claims for both leases should now be cleaned up.
        assert_lease_cleaned_up(&broker.catalog, lease1.id);
        assert_lease_cleaned_up(&broker.catalog, lease2.id);
    }

    #[fuchsia::test]
    fn test_broker_lease_shared_required_element() {
        // Create a topology of one child element with two distinct
        // parents and shared grandparent
        //     > P1
        //   //     \\
        // C           > GP
        //   \\      //
        //     > P2
        //
        // Child ON requires Parent 1 ON and Parent 2 ON.
        // Parent 1 ON requires Grandparent ON.
        // Parent 2 ON requires Grandparent ON.
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));
        let grandparent_token = DependencyToken::create();
        let grandparent: ElementID = broker
            .add_element("GP", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                grandparent,
                grandparent_token
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("dup failed")
                    .into(),
            )
            .expect("register_dependency_token failed");
        let parent1_token = DependencyToken::create();
        let parent1: ElementID = broker
            .add_element(
                "P",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        grandparent_token
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        broker
            .register_dependency_token(
                parent1,
                parent1_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let parent2_token = DependencyToken::create();
        let parent2: ElementID = broker
            .add_element(
                "P",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        grandparent_token
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        broker
            .register_dependency_token(
                parent2,
                parent2_token.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let child = broker
            .add_element(
                "C",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![
                    fpb::LevelDependency {
                        dependent_level: Some(ON.level),
                        requires_token: Some(
                            parent1_token
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .expect("dup failed"),
                        ),
                        requires_level_by_preference: Some(vec![ON.level]),
                        ..Default::default()
                    },
                    fpb::LevelDependency {
                        dependent_level: Some(ON.level),
                        requires_token: Some(
                            parent2_token
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .expect("dup failed"),
                        ),
                        requires_level_by_preference: Some(vec![ON.level]),
                        ..Default::default()
                    },
                ],
            )
            .expect("add_element failed");

        // Initially, all elements should be OFF.
        let mut broker_status = BrokerStatusMatcher::new();
        broker_status.required_level.update(grandparent, OFF);
        broker_status.required_level.update(parent1, OFF);
        broker_status.required_level.update(parent2, OFF);
        broker_status.required_level.update(child, OFF);
        broker_status.assert_matches(&broker);

        // Acquire lease for Child.
        let lease1 =
            broker.acquire_lease(child, ON, zx::Koid::from_raw(1)).expect("acquire failed");
        broker_status.required_level.update(grandparent, ON);
        broker_status.lease.update(lease1.id, LeaseStatus::Pending);
        broker_status.assert_matches(&broker);

        // Raise Grandparent's current level to ON. Now both Parent claims should
        // be enforced, because its dependency on Grandparent is unblocked
        // raising its required level to ON.
        broker.update_current_level(grandparent, ON);
        broker_status.required_level.update(parent1, ON);
        broker_status.required_level.update(parent2, ON);
        broker_status.assert_matches(&broker);

        // Update Parent 1's current level to ON.
        // Child's required level should not increase because Parent 2 is not yet ON.
        broker.update_current_level(parent1, ON);
        broker_status.assert_matches(&broker);

        // Update Parent 2's current level to ON.
        // Child's required level should become ON.
        broker.update_current_level(parent2, ON);
        broker_status.required_level.update(child, ON);
        broker_status.assert_matches(&broker);

        // Update Child's current level to ON.
        // Lease on Child is now satisfied.
        broker.update_current_level(child, ON);
        broker_status.lease.update(lease1.id, LeaseStatus::Satisfied);
        broker_status.assert_matches(&broker);

        // Drop lease for Child.
        // Child's required level should be OFF.
        broker.drop_lease(lease1.id).expect("drop failed");
        broker_status.required_level.update(child, OFF);
        broker_status.lease.remove(lease1.id);
        broker_status.assert_matches(&broker);

        // Lower Child's level to OFF.
        // Parent 1's required level should become OFF.
        // Parent 2's required level should become OFF.
        // Grandparent's required level should not be affected.
        broker.update_current_level(child, OFF);
        broker_status.required_level.update(parent1, OFF);
        broker_status.required_level.update(parent2, OFF);
        broker_status.assert_matches(&broker);

        // Lower Parent 1's level to OFF.
        // Grandparent's required level should not be affected because Parent 2 is still ON.
        broker.update_current_level(parent1, OFF);
        broker_status.assert_matches(&broker);

        // Lower Parent 2's level to OFF.
        // Grandparent's required level should become OFF.
        broker.update_current_level(parent2, OFF);
        broker_status.required_level.update(grandparent, OFF);
        broker_status.assert_matches(&broker);
        assert_lease_cleaned_up(&broker.catalog, lease1.id);
    }

    #[fuchsia::test]
    async fn test_lease_cumulative_implicit_dependency() {
        // Tests that cumulative implicit dependencies are properly resolved when a lease is
        // acquired. Verifies a simple case of dependencies only.
        //
        // A[1] has an dependency on B[1].
        // A[2] has an dependency on C[1].
        //
        // A[2] has an implicit, dependency on B[1].
        //
        //  A     B     C
        //  1 ==> 1
        //  2 ========> 1
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));

        let v012_u8: Vec<u8> = vec![0, 1, 2];

        let token_b = DependencyToken::create();
        let token_c = DependencyToken::create();
        let element_b =
            broker.add_element("B", 0, v012_u8.clone(), vec![]).expect("add_element failed");
        broker
            .register_dependency_token(
                element_b,
                token_b.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let element_c =
            broker.add_element("C", 0, v012_u8.clone(), vec![]).expect("add_element failed");
        broker
            .register_dependency_token(
                element_c,
                token_c.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let element_a = broker
            .add_element(
                "A",
                0,
                v012_u8.clone(),
                vec![
                    fpb::LevelDependency {
                        dependent_level: Some(1),
                        requires_token: Some(
                            token_b
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .expect("dup failed")
                                .into(),
                        ),
                        requires_level_by_preference: Some(vec![1]),
                        ..Default::default()
                    },
                    fpb::LevelDependency {
                        dependent_level: Some(2),
                        requires_token: Some(
                            token_c
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .expect("dup failed")
                                .into(),
                        ),
                        requires_level_by_preference: Some(vec![1]),
                        ..Default::default()
                    },
                ],
            )
            .expect("add_element failed");

        let mut broker_status = BrokerStatusMatcher::new();

        // Initial required levels for all elements should be 0.
        // Set all current levels to 0.
        broker.update_current_level(element_a, ZERO);
        broker.update_current_level(element_b, ZERO);
        broker.update_current_level(element_c, ZERO);
        broker_status.required_level.update(element_a, ZERO);
        broker_status.required_level.update(element_b, ZERO);
        broker_status.required_level.update(element_c, ZERO);
        broker_status.assert_matches(&broker);

        // Lease A[2].
        //
        // A has two dependencies, B[1] and C[1].
        //
        // A's required level should not change.
        // B and C's required level should be 1.
        //
        // A's lease is pending.
        let lease_a =
            broker.acquire_lease(element_a, TWO, zx::Koid::from_raw(1)).expect("acquire failed");
        let lease_a_id = lease_a.id;
        broker_status.required_level.update(element_b, ONE);
        broker_status.required_level.update(element_c, ONE);
        broker_status.assert_matches(&broker);
        assert_eq!(broker.get_lease_status(lease_a.id), Some(LeaseStatus::Pending));

        // Update B, C's current level to 1.
        //
        // A's current level should now be 2.
        // B and C's current level should not change.
        //
        // A's lease should be satisfied.
        broker.update_current_level(element_b, ONE);
        broker.update_current_level(element_c, ONE);
        broker_status.required_level.update(element_a, TWO);
        broker_status.assert_matches(&broker);

        // Update A's current level to 2.
        //
        // A's current level should now be 2.
        // B and C's current level should not change.
        //
        // A's lease should be satisfied.
        broker.update_current_level(element_a, TWO);
        broker_status.lease.update(lease_a.id, LeaseStatus::Satisfied);
        broker_status.assert_matches(&broker);

        // Drop Lease A.
        //
        // A's required level should become 0, as it is no longer leased.
        //
        // Lease A should be pending.
        broker.drop_lease(lease_a.id).expect("drop_lease failed");
        broker_status.required_level.update(element_a, ZERO);
        broker_status.lease.remove(lease_a.id);
        broker_status.assert_matches(&broker);

        // Update A's current level to 0.
        //
        // B's required level should become 0, as A no longer needs it.
        // C's required level should become 0, as A no longer needs it.
        broker.update_current_level(element_a, ZERO);
        broker_status.required_level.update(element_b, ZERO);
        broker_status.required_level.update(element_c, ZERO);
        broker_status.assert_matches(&broker);

        // All leases should be cleaned up.
        assert_lease_cleaned_up(&broker.catalog, lease_a_id);
    }

    #[fuchsia::test]
    fn test_removing_element_permanently_prevents_lease_satisfaction() {
        // Tests that if element A depends on element B, and element B is removed, that new leases
        // on element A will never be satisfied.
        //
        // B has a dependency on A.
        // C has a dependency on A.
        //  A     B     C
        // ON <= ON
        // ON <======= ON
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));
        let token_a = DependencyToken::create();
        let element_a = broker
            .add_element("A", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                element_a,
                token_a.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let element_b = broker
            .add_element(
                "B",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        token_a
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed")
                            .into(),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        let element_c = broker
            .add_element(
                "C",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        token_a
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed")
                            .into(),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");

        let mut broker_status = BrokerStatusMatcher::new();

        // Initial required levels for all elements should be 0.
        // Set all current levels to 0.
        broker.update_current_level(element_a, ZERO);
        broker.update_current_level(element_b, ZERO);
        broker.update_current_level(element_c, ZERO);
        broker_status.required_level.update(element_a, ZERO);
        broker_status.required_level.update(element_b, ZERO);
        broker_status.required_level.update(element_c, ZERO);
        broker_status.assert_matches(&broker);

        // Remove A.
        // B & C's required level should remain OFF.
        broker.remove_element(&element_a);
        broker_status.required_level.remove(element_a);
        broker_status.assert_matches(&broker);

        // Lease B & C.
        // B & C's required level should remain OFF.
        // Both leases should be pending, as they should
        // have a new dependency on the topology unsatisfiable element.
        let lease_b =
            broker.acquire_lease(element_b, ON, zx::Koid::from_raw(1)).expect("acquire failed");
        let lease_c =
            broker.acquire_lease(element_c, ON, zx::Koid::from_raw(2)).expect("acquire failed");
        broker.update_current_level(element_a, ON);
        broker_status.lease.update(lease_b.id, LeaseStatus::Pending);
        broker_status.lease.update(lease_c.id, LeaseStatus::Pending);
        broker_status.assert_matches(&broker);

        broker.drop_lease(lease_b.id).expect("drop_lease failed");
        broker.drop_lease(lease_c.id).expect("drop_lease failed");
        broker_status.lease.remove(lease_b.id);
        broker_status.lease.remove(lease_c.id);
        broker_status.assert_matches(&broker);

        // Leases should be cleaned up.
        assert_lease_cleaned_up(&broker.catalog, lease_b.id);
        assert_lease_cleaned_up(&broker.catalog, lease_c.id);
    }

    #[fuchsia::test]
    fn test_required_level() {
        // Create a topology of one element.
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));
        let element =
            broker.add_element("E", 0, vec![0, 1, 2], vec![]).expect("add_element failed");
        let mut broker_status = BrokerStatusMatcher::new();

        // Initial required level should be 0.
        broker_status.required_level.update(element, ZERO);
        broker_status.assert_matches(&broker);

        // Acquire lease for level 1.
        let lease =
            broker.acquire_lease(element, ONE, zx::Koid::from_raw(1)).expect("acquire failed");
        // Required level should become 1.
        broker_status.required_level.update(element, ONE);
        broker_status.assert_matches(&broker);

        // Drop lease.
        broker.drop_lease(lease.id).expect("drop failed");
        // Required level should stay 1, to preserve orderly transition.
        broker_status.assert_matches(&broker);

        // Update current level to 1 to finish transition.
        broker.update_current_level(element, ONE);
        // Required level should become 0.
        broker_status.required_level.update(element, ZERO);
        broker_status.assert_matches(&broker);

        // Acquire and drop a level 2 lease.
        let lease =
            broker.acquire_lease(element, TWO, zx::Koid::from_raw(2)).expect("acquire failed");
        broker.drop_lease(lease.id).expect("drop failed");
        // Required level should stay 0, to preserve orderly transition.
        broker_status.assert_matches(&broker);

        // Update current level to 0 to finish transition.
        broker.update_current_level(element, ZERO);
        // Required level should remain 0 as the lease has been dropped.
        broker_status.assert_matches(&broker);

        // Acquire lease for level 1 and check required level.
        let lease =
            broker.acquire_lease(element, ONE, zx::Koid::from_raw(3)).expect("acquire failed");
        broker_status.required_level.update(element, ONE);
        broker_status.assert_matches(&broker);

        // Drop lease and check required level.
        broker.drop_lease(lease.id).expect("drop failed");
        // Required level should stay 1, to preserve orderly transition.
        broker_status.assert_matches(&broker);

        // Update current level to 1 to finish transition.
        broker.update_current_level(element, ONE);
        // Required level should become 0.
        broker_status.required_level.update(element, ZERO);
        broker_status.assert_matches(&broker);
    }

    #[fuchsia::test]
    async fn test_add_element_dependency_list_of_levels() {
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));
        let token_mithril = DependencyToken::create();
        let mithril = broker
            .add_element("Mithril", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                mithril,
                token_mithril.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let v01: Vec<u64> = BINARY_POWER_LEVELS.iter().map(|&v| v as u64).collect();

        // Add an element with a dependency with a list of requires_level_by_preference.
        // The dependency should be taken on ON, because the other levels do not
        // exist.
        let silver = broker
            .add_element(
                "Silver",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        token_mithril
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![40, 30, ON.level, 20]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        assert_data_tree!(inspect, root: {
            test: {
                leases: {},
                topology: {
                    events: {
                        "0": {
                            "@time": AnyProperty,
                            add_element: {
                                current_level: "unset",
                                required_level: "unset",
                                element_id: *broker.get_unsatisfiable_element_id(),
                            }
                        },
                        "1": {
                            "@time": AnyProperty,
                            add_element: {
                                current_level: OFF.level as u64,
                                required_level: OFF.level as u64,
                                element_id: *mithril,
                            }
                        },
                        "2": {
                            "@time": AnyProperty,
                            add_element: {
                                current_level: OFF.level as u64,
                                required_level: OFF.level as u64,
                                element_id: *silver,
                                dependencies: {
                                    "0": {
                                        dependent_level: ON.level as u64,
                                        required_element: *mithril,
                                        required_level: ON.level as u64,
                                    }
                                }
                            }
                        },
                    },
                    stats: contains {},
                    "fuchsia.inspect.Graph": {
                        topology: {
                            broker.get_unsatisfiable_element_id().to_string() => {
                                meta: {
                                    name: broker.get_unsatisfiable_element_name(),
                                    valid_levels: broker.get_unsatisfiable_element_levels(),
                                    required_level: "unset",
                                    current_level: "unset",
                                    leases: {}
                                },
                                relationships: {}
                            },
                            mithril.to_string() => {
                                meta: {
                                    name: "Mithril",
                                    valid_levels: v01.clone(),
                                    current_level: OFF.level as u64,
                                    required_level: OFF.level as u64,
                                    leases: {}
                                },
                                relationships: {},
                            },
                            silver.to_string() => {
                                meta: {
                                    name: "Silver",
                                    valid_levels: v01.clone(),
                                    current_level: OFF.level as u64,
                                    required_level: OFF.level as u64,
                                    leases: {}
                                },
                                relationships: {
                                    mithril.to_string() => {
                                        edge_id: AnyProperty,
                                        meta: {
                                            "1": {
                                                required_level: 1u64,
                                            }
                                        }
                                    },
                                },
                            },
                        },
        }}}});
    }

    #[fuchsia::test]
    async fn test_disorderly_element() {
        // Tests that when an element behaves in a disorderly fashion, the broker does
        // not drop the levels of any elements that it depends on, but immediately drops
        // the levels of any elements that depend on it. An element's level change is
        // considered disorderly if it drops its current level even though it has been
        // requested to be at a higher required level.
        //
        // X is our 'disorderly element', it has a dependency on grandparent GP1.
        // X has a dependency on grandparent GP2.
        // P1 (parent) has a dependency on X.
        // P2 (parent) has a dependency on X.
        // C1 and C2 have dependencies on P1.
        // C3 and C4 have dependencies on P2.
        //
        // C1 = P1   GP1
        // C2 //  \\ //
        //          X
        // C4 \\  // \\
        // C3 = P2   GP2
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));

        let token_gp1 = DependencyToken::create();
        let token_gp2 = DependencyToken::create();
        let token_x = DependencyToken::create();
        let token_p1: fidl::Event = DependencyToken::create();
        let token_p2 = DependencyToken::create();

        let element_gp1 = broker
            .add_element("GP1", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                element_gp1,
                token_gp1.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let element_gp2 = broker
            .add_element("GP2", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        broker
            .register_dependency_token(
                element_gp2,
                token_gp2.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let element_x = broker
            .add_element(
                "X",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![
                    fpb::LevelDependency {
                        dependent_level: Some(ON.level),
                        requires_token: Some(
                            token_gp1
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .expect("dup failed")
                                .into(),
                        ),
                        requires_level_by_preference: Some(vec![ON.level]),
                        ..Default::default()
                    },
                    fpb::LevelDependency {
                        dependent_level: Some(ON.level),
                        requires_token: Some(
                            token_gp2
                                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                                .expect("dup failed")
                                .into(),
                        ),
                        requires_level_by_preference: Some(vec![ON.level]),
                        ..Default::default()
                    },
                ],
            )
            .expect("add_element failed");
        broker
            .register_dependency_token(
                element_x,
                token_x.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let element_p1 = broker
            .add_element(
                "P1",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        token_x
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed")
                            .into(),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        broker
            .register_dependency_token(
                element_p1,
                token_p1.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let element_p2 = broker
            .add_element(
                "P2",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        token_x
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed")
                            .into(),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        broker
            .register_dependency_token(
                element_p2,
                token_p2.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");
        let element_c1 = broker
            .add_element(
                "C1",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        token_p1
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed")
                            .into(),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        let element_c2 = broker
            .add_element(
                "C2",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        token_p1
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed")
                            .into(),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        let element_c3 = broker
            .add_element(
                "C3",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        token_p2
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed")
                            .into(),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");
        let element_c4 = broker
            .add_element(
                "C4",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(
                        token_p2
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)
                            .expect("dup failed")
                            .into(),
                    ),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .expect("add_element failed");

        let mut broker_status = BrokerStatusMatcher::new();

        // Grab all required leases and power on all elements.
        let lease_gp2 =
            broker.acquire_lease(element_gp2, ON, zx::Koid::from_raw(1)).expect("acquire failed");
        let lease_c1 =
            broker.acquire_lease(element_c1, ON, zx::Koid::from_raw(2)).expect("acquire failed");
        let lease_c2 =
            broker.acquire_lease(element_c2, ON, zx::Koid::from_raw(3)).expect("acquire failed");
        let lease_c3 =
            broker.acquire_lease(element_c3, ON, zx::Koid::from_raw(4)).expect("acquire failed");
        let lease_c4 =
            broker.acquire_lease(element_c4, ON, zx::Koid::from_raw(5)).expect("acquire failed");
        broker.update_current_level(element_gp1, ON);
        broker.update_current_level(element_gp2, ON);
        broker.update_current_level(element_x, ON);
        broker.update_current_level(element_p1, ON);
        broker.update_current_level(element_p2, ON);
        broker.update_current_level(element_c1, ON);
        broker.update_current_level(element_c2, ON);
        broker.update_current_level(element_c3, ON);
        broker.update_current_level(element_c4, ON);

        // At this point, all elements should have required level ON.
        broker_status.required_level.update(element_gp1, ON);
        broker_status.required_level.update(element_gp2, ON);
        broker_status.required_level.update(element_x, ON);
        broker_status.required_level.update(element_p1, ON);
        broker_status.required_level.update(element_p2, ON);
        broker_status.required_level.update(element_c1, ON);
        broker_status.required_level.update(element_c2, ON);
        broker_status.required_level.update(element_c3, ON);
        broker_status.required_level.update(element_c4, ON);
        broker_status.lease.update(lease_gp2.id, LeaseStatus::Satisfied);
        broker_status.lease.update(lease_c1.id, LeaseStatus::Satisfied);
        broker_status.lease.update(lease_c2.id, LeaseStatus::Satisfied);
        broker_status.lease.update(lease_c3.id, LeaseStatus::Satisfied);
        broker_status.lease.update(lease_c4.id, LeaseStatus::Satisfied);
        broker_status.assert_matches(&broker);

        // Trigger disorderly drop of X.
        // All parents and grandparents of X should immediately drop to OFF.
        // X's required level should also become OFF, to
        // All leases, except for GP2, should now be pending.
        broker.update_current_level(element_x, OFF);
        broker_status.required_level.update(element_p1, OFF);
        broker_status.required_level.update(element_p2, OFF);
        broker_status.required_level.update(element_c1, OFF);
        broker_status.required_level.update(element_c2, OFF);
        broker_status.required_level.update(element_c3, OFF);
        broker_status.required_level.update(element_c4, OFF);
        broker_status.lease.update(lease_c1.id, LeaseStatus::Pending);
        broker_status.lease.update(lease_c2.id, LeaseStatus::Pending);
        broker_status.lease.update(lease_c3.id, LeaseStatus::Pending);
        broker_status.lease.update(lease_c4.id, LeaseStatus::Pending);
        broker_status.assert_matches(&broker);

        // Turn off all parent/child elements to preserve ordering.
        broker.update_current_level(element_p1, OFF);
        broker.update_current_level(element_p2, OFF);
        broker.update_current_level(element_c1, OFF);
        broker.update_current_level(element_c2, OFF);
        broker.update_current_level(element_c3, OFF);
        broker.update_current_level(element_c4, OFF);
        broker_status.assert_matches(&broker);

        // Update X to ON.
        // P1 and P2's required level should become ON.
        broker.update_current_level(element_x, ON);
        broker_status.required_level.update(element_p1, ON);
        broker_status.required_level.update(element_p2, ON);
        broker_status.assert_matches(&broker);

        // Update P1 and P2 to ON.
        broker.update_current_level(element_p1, ON);
        broker.update_current_level(element_p2, ON);
        broker_status.required_level.update(element_c1, ON);
        broker_status.required_level.update(element_c2, ON);
        broker_status.required_level.update(element_c3, ON);
        broker_status.required_level.update(element_c4, ON);
        broker_status.assert_matches(&broker);

        // Update C1, C2, C3, and C4 to ON.
        // All leases should now be satisfied.
        broker.update_current_level(element_c1, ON);
        broker.update_current_level(element_c2, ON);
        broker.update_current_level(element_c3, ON);
        broker.update_current_level(element_c4, ON);
        broker_status.lease.update(lease_c1.id, LeaseStatus::Satisfied);
        broker_status.lease.update(lease_c2.id, LeaseStatus::Satisfied);
        broker_status.lease.update(lease_c3.id, LeaseStatus::Satisfied);
        broker_status.lease.update(lease_c4.id, LeaseStatus::Satisfied);
        broker_status.assert_matches(&broker);

        // Drop all leases, power down children.
        broker.drop_lease(lease_gp2.id).expect("drop_lease failed");
        broker.drop_lease(lease_c1.id).expect("drop_lease failed");
        broker.drop_lease(lease_c2.id).expect("drop_lease failed");
        broker.drop_lease(lease_c3.id).expect("drop_lease failed");
        broker.drop_lease(lease_c4.id).expect("drop_lease failed");
        broker_status.required_level.update(element_c1, OFF);
        broker_status.required_level.update(element_c2, OFF);
        broker_status.required_level.update(element_c3, OFF);
        broker_status.required_level.update(element_c4, OFF);
        broker_status.lease.remove(lease_gp2.id);
        broker_status.lease.remove(lease_c1.id);
        broker_status.lease.remove(lease_c2.id);
        broker_status.lease.remove(lease_c3.id);
        broker_status.lease.remove(lease_c4.id);
        broker_status.assert_matches(&broker);

        // Power down all elements to remove activated claims.
        broker.update_current_level(element_c1, OFF);
        broker.update_current_level(element_c2, OFF);
        broker.update_current_level(element_c3, OFF);
        broker.update_current_level(element_c4, OFF);
        broker_status.required_level.update(element_p1, OFF);
        broker_status.required_level.update(element_p2, OFF);
        broker_status.assert_matches(&broker);

        broker.update_current_level(element_p1, OFF);
        broker.update_current_level(element_p2, OFF);
        broker_status.required_level.update(element_x, OFF);
        broker_status.assert_matches(&broker);

        broker.update_current_level(element_x, OFF);
        broker_status.required_level.update(element_gp1, OFF);
        broker_status.required_level.update(element_gp2, OFF);
        broker_status.assert_matches(&broker);

        // All leases should be cleaned up.
        assert_lease_cleaned_up(&broker.catalog, lease_gp2.id);
        assert_lease_cleaned_up(&broker.catalog, lease_c1.id);
        assert_lease_cleaned_up(&broker.catalog, lease_c2.id);
        assert_lease_cleaned_up(&broker.catalog, lease_c3.id);
        assert_lease_cleaned_up(&broker.catalog, lease_c4.id);
    }

    #[fuchsia::test]
    async fn test_direct_lease() {
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));

        // B has a dependency on A.
        // C has a dependency on D.
        // We will directly lease both B and C.
        // A <= B <= L => C => D
        let element_a =
            broker.add_element("A", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![]).unwrap();
        let token_a = DependencyToken::create();
        broker
            .register_dependency_token(
                element_a,
                token_a.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap().into(),
            )
            .unwrap();

        let element_b = broker
            .add_element(
                "B",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(token_a.into()),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .unwrap();
        let token_b = DependencyToken::create();
        broker
            .register_dependency_token(
                element_b,
                token_b.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap().into(),
            )
            .unwrap();

        let element_d =
            broker.add_element("D", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![]).unwrap();
        let token_d = DependencyToken::create();
        broker
            .register_dependency_token(
                element_d,
                token_d.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap().into(),
            )
            .unwrap();

        let element_c = broker
            .add_element(
                "C",
                OFF.level,
                BINARY_POWER_LEVELS.to_vec(),
                vec![fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(token_d.into()),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                }],
            )
            .unwrap();
        let token_c = DependencyToken::create();
        broker
            .register_dependency_token(
                element_c,
                token_c.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap().into(),
            )
            .unwrap();

        // Direct Lease L depends on B and C.
        let dependencies = vec![
            fpb::LeaseDependency {
                requires_token: Some(token_b.into()),
                requires_level_by_preference: Some(vec![ON.level]),
                ..Default::default()
            },
            fpb::LeaseDependency {
                requires_token: Some(token_c.into()),
                requires_level_by_preference: Some(vec![ON.level]),
                ..Default::default()
            },
        ];

        let lease_token_koid = zx::Koid::from_raw(123);
        let lease =
            broker.acquire_direct_lease("L".to_string(), dependencies, lease_token_koid).unwrap();

        // Initially, only root elements (A and D) should have their required levels updated.
        // B and C depend on A and D respectively, so they are not yet activated.
        assert_eq!(broker.get_required_level(&element_a), Some(ON));
        assert_eq!(broker.get_required_level(&element_d), Some(ON));
        assert_eq!(broker.get_required_level(&element_b), Some(OFF));
        assert_eq!(broker.get_required_level(&element_c), Some(OFF));

        // Lease should be Pending.
        assert_eq!(broker.get_lease_status(lease.id), Some(LeaseStatus::Pending));

        // Now satisfy A and D.
        broker.update_current_level(element_a, ON);
        broker.update_current_level(element_d, ON);

        // Now B and C should have their required levels updated to ON.
        assert_eq!(broker.get_required_level(&element_b), Some(ON));
        assert_eq!(broker.get_required_level(&element_c), Some(ON));
        assert_eq!(broker.get_lease_status(lease.id), Some(LeaseStatus::Pending));

        // Now satisfy B and C.
        broker.update_current_level(element_b, ON);
        broker.update_current_level(element_c, ON);
        // Now all dependencies are satisfied.
        assert_eq!(broker.get_lease_status(lease.id), Some(LeaseStatus::Satisfied));

        // Drop the lease.
        broker.drop_lease(lease.id).unwrap();
        // B and C's required levels should immediately drop to OFF.
        assert_eq!(broker.get_required_level(&element_b), Some(OFF));
        assert_eq!(broker.get_required_level(&element_c), Some(OFF));
        // A and D's required levels should remain ON.
        assert_eq!(broker.get_required_level(&element_a), Some(ON));
        assert_eq!(broker.get_required_level(&element_d), Some(ON));

        // Power down B and C.
        broker.update_current_level(element_b, OFF);
        broker.update_current_level(element_c, OFF);

        // Now A and D's required levels levels should drop to OFF.
        assert_eq!(broker.get_required_level(&element_a), Some(OFF));
        assert_eq!(broker.get_required_level(&element_d), Some(OFF));
        // B and C's required levels should remain OFF.
        assert_eq!(broker.get_required_level(&element_b), Some(OFF));
        assert_eq!(broker.get_required_level(&element_c), Some(OFF));
    }

    #[fuchsia::test]
    fn test_update_leases_for_dependency_active() {
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));

        let element_a = broker
            .add_element("A", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        let element_b = broker
            .add_element("B", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");

        // Create a lease for A at ON and verify that A's required level is now ON.
        let lease =
            broker.acquire_lease(element_a, ON, zx::Koid::from_raw(1)).expect("acquire failed");
        assert_eq!(broker.get_required_level(&element_a), Some(ON));

        // Update current level to ON and verify the lease is satisfied.
        broker.update_current_level(element_a, ON);
        assert_eq!(broker.get_lease_status(lease.id), Some(LeaseStatus::Satisfied));
        let pending_claims: Vec<_> = broker.catalog.claims.pending.for_lease(lease.id).collect();
        assert_eq!(pending_claims.len(), 0);
        let activated_claims: Vec<_> =
            broker.catalog.claims.activated.for_lease(lease.id).collect();
        assert_eq!(activated_claims.len(), 1);
        let claim_a = activated_claims[0];
        assert_eq!(claim_a.dependency.requires.element_id, element_a);
        assert_eq!(claim_a.dependency.requires.level, ON);

        // Now add a dependency: A at ON requires B at ON.
        let dependency = Dependency {
            dependent: ElementLevel { element_id: element_a, level: ON },
            requires: ElementLevel { element_id: element_b, level: ON },
        };
        broker.update_leases_for_dependency(dependency.clone());

        // Verify that a claim for B at ON is added to the lease (and no other claims were added).
        let pending_claims: Vec<_> = broker.catalog.claims.pending.for_lease(lease.id).collect();
        assert_eq!(pending_claims.len(), 0);
        let activated_claims: Vec<_> =
            broker.catalog.claims.activated.for_lease(lease.id).collect();
        assert_eq!(activated_claims.len(), 2);

        // One claim should be for A at ON and one for B at ON.
        let found_a = activated_claims.iter().any(|c| {
            c.dependency.requires.element_id == element_a && c.dependency.requires.level == ON
        });
        let found_b = activated_claims.iter().any(|c| c.dependency == dependency);
        assert!(found_a, "Claim for A at ON not found in activated claims");
        assert!(found_b, "Claim for B at ON not found in activated claims");
    }

    #[fuchsia::test]
    fn test_update_leases_for_dependency_unaffected() {
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));

        let levels = vec![0, 1, 2];
        let level_0 = IndexedPowerLevel { level: 0, index: 0 };
        let level_1 = IndexedPowerLevel { level: 1, index: 1 };
        let level_2 = IndexedPowerLevel { level: 2, index: 2 };

        let element_a = broker
            .add_element("A", level_0.level, levels.clone(), vec![])
            .expect("add_element failed");
        let element_b = broker
            .add_element("B", level_0.level, levels.clone(), vec![])
            .expect("add_element failed");

        // Create a lease for A at 1.
        let lease = broker
            .acquire_lease(element_a, level_1, zx::Koid::from_raw(1))
            .expect("acquire failed");

        // There should be only one claim for A at 1.
        let pending_claims: Vec<_> = broker.catalog.claims.pending.for_lease(lease.id).collect();
        let activated_claims: Vec<_> =
            broker.catalog.claims.activated.for_lease(lease.id).collect();
        let all_claims: Vec<_> =
            pending_claims.into_iter().chain(activated_claims.into_iter()).collect();
        assert_eq!(all_claims.len(), 1);
        assert_eq!(all_claims[0].dependency.requires.element_id, element_a);
        assert_eq!(all_claims[0].dependency.requires.level, level_1);

        // Now add dependency: A at 2 requires B at 1.
        // Since A is only leased at 1, the lease should not be affected.
        let dependency = Dependency {
            dependent: ElementLevel { element_id: element_a, level: level_2 },
            requires: ElementLevel { element_id: element_b, level: level_1 },
        };
        broker.update_leases_for_dependency(dependency);

        // Verify that NO new claim is added.
        let pending_claims: Vec<_> = broker.catalog.claims.pending.for_lease(lease.id).collect();
        let activated_claims: Vec<_> =
            broker.catalog.claims.activated.for_lease(lease.id).collect();
        let all_claims: Vec<_> =
            pending_claims.into_iter().chain(activated_claims.into_iter()).collect();
        assert_eq!(all_claims.len(), 1);
        assert_eq!(all_claims[0].dependency.requires.element_id, element_a);
        assert_eq!(all_claims[0].dependency.requires.level, level_1);
    }

    #[fuchsia::test]
    fn test_removable_dependency() {
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));

        let token_a = DependencyToken::create();
        let element_a = broker.add_element("A", 0, vec![0, 1], vec![]).expect("add_element failed");
        broker
            .register_dependency_token(
                element_a,
                token_a.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register failed");

        let dep = fpb::LevelDependency {
            dependent_level: Some(1),
            requires_token: Some(
                token_a.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
            ),
            requires_level_by_preference: Some(vec![1]),
            remove_with_required_element: Some(true),
            ..Default::default()
        };

        let element_b =
            broker.add_element("B", 0, vec![0, 1], vec![dep]).expect("add_element failed");

        let lease = broker
            .acquire_lease(
                element_b,
                IndexedPowerLevel { level: 1, index: 1 },
                zx::Koid::from_raw(1),
            )
            .expect("acquire failed");

        // B's lease is pending A(1) being satisfied. B's required level is still 0.
        assert_eq!(broker.get_required_level(&element_b).unwrap().level, 0);
        // A's required level has been raised to 1 due to B's dependency.
        assert_eq!(broker.get_required_level(&element_a).unwrap().level, 1);

        // Transition A to 1.
        broker.update_current_level(element_a, IndexedPowerLevel { level: 1, index: 1 });

        // Now B's dependency is satisfied, so B's required level is 1.
        assert_eq!(broker.get_required_level(&element_b).unwrap().level, 1);

        // Transition B to 1 to satisfy the lease.
        broker.update_current_level(element_b, IndexedPowerLevel { level: 1, index: 1 });
        assert_eq!(broker.get_lease_status(lease.id), Some(LeaseStatus::Satisfied));

        broker.remove_element(&element_a);

        // Since the dependency was removable, B's required level should not be dropped to minimum/unsatisfiable.
        assert_eq!(broker.get_required_level(&element_b).unwrap().level, 1);
        // The lease should still be satisfied.
        assert_eq!(broker.get_lease_status(lease.id), Some(LeaseStatus::Satisfied));
    }

    #[fuchsia::test]
    fn test_lease_powering_down_and_vacated_states() {
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));

        let element_a = broker
            .add_element("A", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");
        let element_b = broker
            .add_element("B", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");

        let token_b = DependencyToken::create();
        broker
            .register_dependency_token(
                element_b,
                token_b.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");

        // A at ON requires B at ON.
        broker
            .add_dependency(
                element_a,
                fpb::LevelDependency {
                    dependent_level: Some(ON.level),
                    requires_token: Some(token_b),
                    requires_level_by_preference: Some(vec![ON.level]),
                    ..Default::default()
                },
                &mut EagerInspectWriter,
            )
            .expect("add_dependency failed");

        // Acquire lease on A at ON.
        let lease =
            broker.acquire_lease(element_a, ON, zx::Koid::from_raw(1)).expect("acquire failed");

        // Satisfy dependencies.
        broker.update_current_level(element_b, ON);
        broker.update_current_level(element_a, ON);

        // Verify lease is Satisfied.
        assert_eq!(broker.get_lease_status(lease.id), Some(LeaseStatus::Satisfied));
        assert!(broker.catalog.leases.contains_key(&lease.id));

        // Drop the lease.
        broker.drop_lease(lease.id).expect("drop failed");

        // Verify it enters POWERING_DOWN state and is not yet deleted.
        assert_eq!(broker.get_lease_status(lease.id), Some(LeaseStatus::PoweringDown));
        assert!(broker.catalog.leases.contains_key(&lease.id));
        assert!(broker.catalog.topology.elements.contains_key(&lease.synthetic_element_id));

        // Let's power down element A to OFF. This should cascade and drop claims.
        broker.update_current_level(element_a, OFF);

        // The lease should be VACATED now, but we can't observe that as it has
        // been cleaned up.
        assert_eq!(broker.get_lease_status(lease.id), None);
        assert!(!broker.catalog.leases.contains_key(&lease.id));
        assert!(!broker.catalog.topology.elements.contains_key(&lease.synthetic_element_id));
    }

    #[fuchsia::test]
    fn test_stranded_powering_down_lease() {
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));

        let element_a = broker
            .add_element("A", OFF.level, vec![0, 10, 20], vec![])
            .expect("add_element failed");
        let element_b = broker
            .add_element("B", OFF.level, vec![0, 10, 20], vec![])
            .expect("add_element failed");

        let token_b = DependencyToken::create();
        broker
            .register_dependency_token(
                element_b,
                token_b.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed").into(),
            )
            .expect("register_dependency_token failed");

        // A     B
        // 20 -> 20
        // 10 -> 10
        broker
            .add_dependency(
                element_a,
                fpb::LevelDependency {
                    dependent_level: Some(10),
                    requires_token: Some(
                        token_b.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("dup failed"),
                    ),
                    requires_level_by_preference: Some(vec![10]),
                    ..Default::default()
                },
                &mut EagerInspectWriter,
            )
            .expect("add_dependency failed");
        broker
            .add_dependency(
                element_a,
                fpb::LevelDependency {
                    dependent_level: Some(20),
                    requires_token: Some(token_b),
                    requires_level_by_preference: Some(vec![20]),
                    ..Default::default()
                },
                &mut EagerInspectWriter,
            )
            .expect("add_dependency failed");

        let level_10 = IndexedPowerLevel { level: 10, index: 1 };
        let level_20 = IndexedPowerLevel { level: 20, index: 2 };

        // Acquire Lease 1 on A at level 10.
        let lease1 =
            broker.acquire_lease(element_a, level_10.clone(), zx::Koid::from_raw(1)).unwrap();
        broker.update_current_level(element_b, level_10.clone());
        broker.update_current_level(element_a, level_10.clone());
        assert_eq!(broker.get_lease_status(lease1.id), Some(LeaseStatus::Satisfied));

        // Acquire Lease 2 on A at level 10.
        let lease2 =
            broker.acquire_lease(element_a, level_10.clone(), zx::Koid::from_raw(2)).unwrap();
        assert_eq!(broker.get_lease_status(lease2.id), Some(LeaseStatus::Satisfied));

        // Bring B to level 20 first so that Lease 3's top claim can be
        // activated immediately.
        broker.update_current_level(element_b, level_20.clone());

        // Acquire Lease 3 on A at level 20. This puts element A into
        // in_transition while transiting to 20.
        let lease3 =
            broker.acquire_lease(element_a, level_20.clone(), zx::Koid::from_raw(3)).unwrap();

        // While A is in transition, drop Lease 2.
        // Because A is in transition, Lease 2's bottom claim (A@10 -> B@10)
        // cannot be dropped safely, leaving Lease 2 stranded in PoweringDown
        // until the transition is complete.
        broker.drop_lease(lease2.id).unwrap();
        assert_eq!(broker.get_lease_status(lease2.id), Some(LeaseStatus::PoweringDown));

        // Complete transition of A to level 20.
        broker.update_current_level(element_a, level_20.clone());
        assert_eq!(broker.get_lease_status(lease3.id), Some(LeaseStatus::Satisfied));

        // Now that A is at level 20 and Lease 1 and Lease 3 are SATISFIED,
        // Lease 2's claims are redundantly covered and Lease 2 is cleanly
        // vacated (None).
        assert_eq!(broker.get_lease_status(lease2.id), None);

        // Now drop Lease 3 and step A back down to level 10.
        broker.drop_lease(lease3.id).unwrap();
        broker.update_current_level(element_a, level_10.clone());
        assert_eq!(broker.get_lease_status(lease1.id), Some(LeaseStatus::Satisfied));
        assert_eq!(broker.get_lease_status(lease3.id), None);
    }

    #[fuchsia::test]
    fn test_overlapping_leases_vacate_immediately() {
        let inspect = fuchsia_inspect::Inspector::default();
        let mut broker = Broker::new(inspect.root().create_child("test"));

        let element_a = broker
            .add_element("A", OFF.level, BINARY_POWER_LEVELS.to_vec(), vec![])
            .expect("add_element failed");

        let lease1 =
            broker.acquire_lease(element_a, ON, zx::Koid::from_raw(1)).expect("acquire failed");
        let lease2 =
            broker.acquire_lease(element_a, ON, zx::Koid::from_raw(2)).expect("acquire failed");

        broker.update_current_level(element_a, ON);
        assert_eq!(broker.get_lease_status(lease1.id), Some(LeaseStatus::Satisfied));
        assert_eq!(broker.get_lease_status(lease2.id), Some(LeaseStatus::Satisfied));

        // Dropping Lease 1 while Lease 2 is active should immediately vacate Lease 1
        // because Lease 2's active claim supports element_a at ON.
        broker.drop_lease(lease1.id).expect("drop failed");
        assert_eq!(broker.get_lease_status(lease1.id), None);
        assert!(!broker.catalog.leases.contains_key(&lease1.id));
        assert!(!broker.catalog.topology.elements.contains_key(&lease1.synthetic_element_id));

        assert_eq!(broker.get_lease_status(lease2.id), Some(LeaseStatus::Satisfied));
    }
}
