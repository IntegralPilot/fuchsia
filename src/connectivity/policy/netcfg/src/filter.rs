// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;

use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_deprecated as fnet_filter_deprecated;
use fidl_fuchsia_net_filter_ext::{
    self as fnet_filter_ext, Action, Change, CommitError, Domain, InstalledIpRoutine,
    InstalledNatRoutine, IpHook, Matchers, Namespace, NamespaceId, NatHook, PushChangesError,
    Resource, ResourceId, Routine, RoutineId, RoutineType, Rule, RuleId,
};
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_masquerade as fnet_masquerade;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use fuchsia_async::DurationExt as _;

use anyhow::{Context as _, bail};
use log::{error, info, warn};

use crate::{FilterConfig, InterfaceId, InterfaceType, exit_with_fidl_error};

/// An error observed on the `fuchsia.net.filter` API.
#[derive(Debug)]
pub(crate) enum FilterError {
    Push(PushChangesError),
    Commit(CommitError),
}

// A container to dispatch filtering functions depending on the
// filtering API present.
pub(crate) enum FilterControl {
    Deprecated(fnet_filter_deprecated::FilterProxy),
    Current(Box<FilterState>),
}

impl FilterControl {
    // Determine whether to use the fuchsia.net.filter.deprecated API or the
    // fuchsia.net.filter API. When the deprecated API is present and active,
    // we should use it.
    pub(super) async fn new(
        deprecated_proxy: Option<fnet_filter_deprecated::FilterProxy>,
        current_proxy: Option<fnet_filter::ControlProxy>,
    ) -> Result<Self, anyhow::Error> {
        if let Some(proxy) = deprecated_proxy {
            if probe_for_presence(&proxy).await {
                return Ok(FilterControl::Deprecated(proxy));
            }
        }

        if let Some(proxy) = current_proxy {
            let controller_id = fnet_filter_ext::ControllerId(String::from("netcfg"));
            let filter = FilterControl::Current(Box::new(FilterState {
                controller: fnet_filter_ext::Controller::new(&proxy, &controller_id)
                    .await
                    .context("could not create controller from filter proxy")?,
                uninstalled_ip_routines: filter_routines(false /* installed */),
                installed_ip_routines: filter_routines(true /* installed */),
                current_installed_rule_index: 0,
                masquerade: MasqueradeState {
                    routine_id: masquerade_routine(),
                    next_rule_index: 0,
                },
            }));
            return Ok(filter);
        }

        Err(anyhow::anyhow!("no filtering proxy available!"))
    }

    /// Updates the initial network filter configuration using either
    /// fuchsia.net.filter.deprecated or fuchsia.net.filter.
    pub(super) async fn update_filters(
        &mut self,
        config: FilterConfig,
        filter_enabled_state: &FilterEnabledState,
    ) -> Result<(), anyhow::Error> {
        match self {
            FilterControl::Deprecated(proxy) => update_filters_deprecated(proxy, config).await,
            FilterControl::Current(state) => {
                state.update_filters_current(config, filter_enabled_state).await
            }
        }
    }
}

// Filtering state for Masquerade NAT on the current `fuchsia.net.filter` API.
struct MasqueradeState {
    // The routine that holds all masquerade rules.
    routine_id: RoutineId,
    // The index to use for the next masquerade rule.
    //
    // Note: By using a simple counter, we don't re-use indices that were once
    // used but are now available. The upside to this approach is that all
    // filtering config has a stable order: older filtering config will always
    // have a lower index (and therefore a higher priority) than newer filtering
    // config. On the other hand, we do run the risk of overflowing the index
    // if Netcfg were to add/remove u32::MAX filtering rules. That should only
    // happen under pathological circumstances, and thus is a non-concern.
    next_rule_index: u32,
}

// Filtering state on the current `fuchsia.net.filter` API.
pub(super) struct FilterState {
    controller: fnet_filter_ext::Controller,
    uninstalled_ip_routines: netfilter::parser::FilterRoutines,
    installed_ip_routines: netfilter::parser::FilterRoutines,
    masquerade: MasqueradeState,
    current_installed_rule_index: u32,
    // TODO(https://fxbug.dev/331469354): Add NAT routines when this
    // functionality has been added to fuchsia.net.filter.
}

impl FilterState {
    // Commit the initial filter state using fuchsia.net.filter.
    async fn update_filters_current(
        &mut self,
        config: FilterConfig,
        filter_enabled_state: &FilterEnabledState,
    ) -> Result<(), anyhow::Error> {
        let FilterState {
            controller,
            uninstalled_ip_routines,
            installed_ip_routines,
            current_installed_rule_index,
            masquerade,
        } = self;
        let (changes, num_installed_rules) = generate_initial_filter_changes(
            uninstalled_ip_routines,
            installed_ip_routines,
            &masquerade.routine_id,
            config,
            &filter_enabled_state.interface_types,
            *current_installed_rule_index,
        )?;

        for batch in changes.chunks(usize::from(fnet_filter::MAX_BATCH_SIZE)) {
            controller
                .push_changes(batch.to_vec())
                .await
                .context("failed to push changes to filter controller")?;
        }

        controller.commit().await.context("failed to commit changes to filter controller")?;
        info!("initial filter configuration has been committed successfully");
        *current_installed_rule_index =
            current_installed_rule_index.wrapping_add(num_installed_rules);
        Ok(())
    }
}

// Netcfg's `FilterRoutines` to maintain the same namespace
// and routine for each of the filter `Rule`s across installed
// and uninstalled routines.
fn filter_routines(installed: bool) -> netfilter::parser::FilterRoutines {
    let suffix = if !installed { "_uninstalled" } else { "" };
    netfilter::parser::FilterRoutines {
        local_ingress: Some(RoutineId {
            namespace: namespace_id(),
            name: format!("local_ingress{suffix}"),
        }),
        local_egress: Some(RoutineId {
            namespace: namespace_id(),
            name: format!("local_egress{suffix}"),
        }),
    }
}

// Netcfg's masquerade NAT `RoutineId`.
//
// Masquerade NAT rules are always installed at the EGRESS hook.
fn masquerade_routine() -> RoutineId {
    RoutineId { namespace: namespace_id(), name: format!("egress_masquerade") }
}

fn namespace_id() -> NamespaceId {
    NamespaceId(String::from("netcfg"))
}

pub(super) async fn probe_for_presence(filter: &fnet_filter_deprecated::FilterProxy) -> bool {
    match filter.check_presence().await {
        Ok(()) => true,
        Err(fidl::Error::ClientChannelClosed { .. }) => false,
        Err(e) => panic!("unexpected error while probing: {e}"),
    }
}

// Create a set of `fnet_filter_ext::Change`s that, when used with
// `fnet_filter_ext::Controller`, will establish the initial filtering
// state for the `netcfg` namespace.
fn generate_initial_filter_changes(
    uninstalled_ip_routines: &netfilter::parser::FilterRoutines,
    installed_ip_routines: &netfilter::parser::FilterRoutines,
    masquerade_routine: &RoutineId,
    config: FilterConfig,
    filter_enabled_interface_types: &HashSet<InterfaceType>,
    current_installed_rule_index: u32,
) -> Result<(Vec<Change>, u32), anyhow::Error> {
    let namespace = Change::Create(Resource::Namespace(Namespace {
        id: NamespaceId(String::from("netcfg")),
        domain: Domain::AllIp,
    }));
    let mut changes = vec![namespace];

    // Create uninstalled `Routine`s that the installed `Routine`s can use to
    // `Jump` to `Rule`s. There must be a separate uninstalled `Routine` for
    // each `IpHook` so that there are not issues with a `Rule` containing
    // a matcher that is not allowed in the installed `Routine`'s hook.
    // E.g., A `Rule` in an installed ingress hook `Routine` that `Jump`s to a
    // `Routine` with a `Rule` that specifies an out_interface matcher is not
    // permitted.
    let netfilter::parser::FilterRoutines { local_ingress, local_egress } = uninstalled_ip_routines;
    let uninstalled_local_ingress =
        local_ingress.clone().map(|id| Routine { id, routine_type: RoutineType::Ip(None) });
    let uninstalled_local_egress =
        local_egress.clone().map(|id| Routine { id, routine_type: RoutineType::Ip(None) });

    // Push installed routines so that netcfg can install `Jump` rules
    // at interface installation time that are rooted in these routines.
    fn installed_routine_from_id(id: RoutineId, hook: IpHook) -> Routine {
        Routine {
            id: id,
            routine_type: RoutineType::Ip(Some(InstalledIpRoutine { hook, priority: 0i32 })),
        }
    }
    let netfilter::parser::FilterRoutines { local_ingress, local_egress } = installed_ip_routines;
    let local_ingress =
        local_ingress.clone().map(|id| installed_routine_from_id(id, IpHook::LocalIngress));
    let local_egress =
        local_egress.clone().map(|id| installed_routine_from_id(id, IpHook::LocalEgress));

    let masquerade = Routine {
        id: masquerade_routine.clone(),
        routine_type: RoutineType::Nat(Some(InstalledNatRoutine {
            hook: NatHook::Egress,
            priority: 0i32,
        })),
    };

    let routine_changes = [
        uninstalled_local_ingress,
        local_ingress,
        uninstalled_local_egress,
        local_egress,
        Some(masquerade),
    ]
    .into_iter()
    .filter_map(|routine| routine)
    .map(|routine| Change::Create(Resource::Routine(routine)));
    changes.extend(routine_changes);

    // TODO(https://fxbug.dev/331469354): Handle NAT and NAT RDR rules when supported
    // by netfilter and filtering library
    let FilterConfig { rules, nat_rules: _, rdr_rules: _ } = config;
    if !rules.is_empty() {
        // Only insert the rules from the config into the uninstalled routine.
        // The rules inserted in the installed routines will be intended for
        // redirection to the uninstalled routines.
        let rules =
            netfilter::parser::parse_str_to_rules(&rules.join(""), &uninstalled_ip_routines)
                .context("error parsing filter rules")?;
        let rule_changes = rules.into_iter().map(|rule| Change::Create(Resource::Rule(rule)));
        changes.extend(rule_changes);
    }

    // TODO(https://fxbug.dev/530218539): Add PortClass filtering for
    // interfaces other than LoWPAN once NS2/filter.deprecated is removed.
    let mut num_installed_rules = 0;
    if filter_enabled_interface_types.contains(&InterfaceType::Lowpan) {
        let lowpan_rules = generate_static_port_class_filter_rules(
            uninstalled_ip_routines,
            installed_ip_routines,
            fnet_interfaces_ext::PortClass::Lowpan,
            current_installed_rule_index.wrapping_add(num_installed_rules),
        );
        changes.extend(lowpan_rules.into_iter().map(|rule| Change::Create(Resource::Rule(rule))));
        num_installed_rules += 1;
    }

    Ok((changes, num_installed_rules))
}

// Create a list of `fnet_filter_ext::Rule`s that, when used with
// `fnet_filter_ext::Controller`, will `Jump` on each available
// `IpHook` to the corresponding uninstalled routine for
// that `IpHook`.
fn generate_updated_filter_rules(
    uninstalled_ip_routines: &netfilter::parser::FilterRoutines,
    installed_ip_routines: &netfilter::parser::FilterRoutines,
    interface_id: InterfaceId,
    current_installed_rule_index: u32,
) -> Vec<Rule> {
    let netfilter::parser::FilterRoutines {
        local_ingress: uninstalled_local_ingress,
        local_egress: uninstalled_local_egress,
    } = uninstalled_ip_routines;
    let netfilter::parser::FilterRoutines { local_ingress, local_egress } = installed_ip_routines;

    // Use the same rule index for all rules created for the
    // interface. It is assumed that all `Rule`s across the
    // `FilterRoutines` will inserted in tandem.
    let local_ingress_rule = local_ingress.clone().map(|routine_id| {
        create_interface_matching_jump_rule(
            routine_id,
            current_installed_rule_index,
            interface_id,
            IpHook::LocalIngress,
            &uninstalled_local_ingress
                .as_ref()
                .expect("there should be a corresponding uninstalled routine for local ingress")
                .name,
        )
    });
    let local_egress_rule = local_egress.clone().map(|routine_id| {
        create_interface_matching_jump_rule(
            routine_id,
            current_installed_rule_index,
            interface_id,
            IpHook::LocalEgress,
            &uninstalled_local_egress
                .as_ref()
                .expect("there should be a corresponding uninstalled routine for local egress")
                .name,
        )
    });

    [local_ingress_rule, local_egress_rule].into_iter().flatten().collect()
}

fn create_jump_rule(
    routine_id: RoutineId,
    index: u32,
    interface: fnet_matchers_ext::Interface,
    hook: IpHook,
    target_routine_name: &str,
) -> Rule {
    let (in_interface, out_interface) = match hook {
        IpHook::LocalIngress | IpHook::Ingress => (Some(interface), None),
        IpHook::LocalEgress | IpHook::Egress => (None, Some(interface)),
        IpHook::Forwarding => (Some(interface.clone()), Some(interface)),
    };

    Rule {
        id: RuleId { routine: routine_id, index },
        matchers: Matchers { in_interface, out_interface, ..Default::default() },
        action: Action::Jump(target_routine_name.to_string()),
    }
}

fn create_interface_matching_jump_rule(
    routine_id: RoutineId,
    index: u32,
    interface_id: InterfaceId,
    hook: IpHook,
    target_routine_name: &str,
) -> Rule {
    create_jump_rule(
        routine_id,
        index,
        fnet_matchers_ext::Interface::Id(interface_id.into()),
        hook,
        target_routine_name,
    )
}

/// Generates static filter rules (jump rules) for a given `PortClass` (specifically Lowpan) to
/// redirect traffic to uninstalled routines.
fn generate_static_port_class_filter_rules(
    uninstalled_ip_routines: &netfilter::parser::FilterRoutines,
    installed_ip_routines: &netfilter::parser::FilterRoutines,
    port_class: fnet_interfaces_ext::PortClass,
    current_installed_rule_index: u32,
) -> Vec<Rule> {
    let netfilter::parser::FilterRoutines {
        local_ingress: uninstalled_local_ingress,
        local_egress: uninstalled_local_egress,
    } = uninstalled_ip_routines;
    let netfilter::parser::FilterRoutines { local_ingress, local_egress } = installed_ip_routines;

    let local_ingress_rule = local_ingress.clone().map(|routine_id| {
        create_port_class_matching_jump_rule(
            routine_id,
            current_installed_rule_index,
            port_class,
            IpHook::LocalIngress,
            &uninstalled_local_ingress
                .as_ref()
                .expect("there should be a corresponding uninstalled routine for local ingress")
                .name,
        )
    });
    let local_egress_rule = local_egress.clone().map(|routine_id| {
        create_port_class_matching_jump_rule(
            routine_id,
            current_installed_rule_index,
            port_class,
            IpHook::LocalEgress,
            &uninstalled_local_egress
                .as_ref()
                .expect("there should be a corresponding uninstalled routine for local egress")
                .name,
        )
    });

    [local_ingress_rule, local_egress_rule].into_iter().flatten().collect()
}

/// Helper to create a single rule matching a `PortClass` on input/output interfaces depending on
/// the hook.
fn create_port_class_matching_jump_rule(
    routine_id: RoutineId,
    index: u32,
    port_class: fnet_interfaces_ext::PortClass,
    hook: IpHook,
    target_routine_name: &str,
) -> Rule {
    create_jump_rule(
        routine_id,
        index,
        fnet_matchers_ext::Interface::PortClass(port_class),
        hook,
        target_routine_name,
    )
}

// We use Compare-And-Swap (CAS) protocol to update filter rules. $get_rules returns the current
// generation number. $update_rules will send it with new rules to make sure we are updating the
// intended generation. If the generation number doesn't match, $update_rules will return a
// GenerationMismatch error, then we have to restart from $get_rules.

pub(crate) const FILTER_CAS_RETRY_MAX: i32 = 3;
pub(crate) const FILTER_CAS_RETRY_INTERVAL_MILLIS: i64 = 500;

macro_rules! cas_filter_rules {
    ($filter:expr, $get_rules:ident, $update_rules:ident, $rules:expr, $error_type:ident) => {
        for retry in 0..FILTER_CAS_RETRY_MAX {
            let (_rules, generation) =
                $filter.$get_rules().await.unwrap_or_else(|err| exit_with_fidl_error(err));

            match $filter
                .$update_rules(&$rules, generation)
                .await
                .unwrap_or_else(|err| exit_with_fidl_error(err))
            {
                Ok(()) => {
                    break;
                }
                Err(fnet_filter_deprecated::$error_type::GenerationMismatch)
                    if retry < FILTER_CAS_RETRY_MAX - 1 =>
                {
                    fuchsia_async::Timer::new(
                        zx::MonotonicDuration::from_millis(FILTER_CAS_RETRY_INTERVAL_MILLIS)
                            .after_now(),
                    )
                    .await;
                }
                Err(e) => {
                    bail!("{} failed: {:?}", stringify!($update_rules), e);
                }
            }
        }
    };
}

// This is a placeholder macro while some update operations are not supported.
macro_rules! no_update_filter_rules {
    ($filter:expr, $get_rules:ident, $update_rules:ident, $rules:expr, $error_type:ident) => {
        let (_rules, generation) =
            $filter.$get_rules().await.unwrap_or_else(|err| exit_with_fidl_error(err));

        match $filter
            .$update_rules(&$rules, generation)
            .await
            .unwrap_or_else(|err| exit_with_fidl_error(err))
        {
            Ok(()) => {}
            Err(fnet_filter_deprecated::$error_type::NotSupported) => {
                error!("{} not supported", stringify!($update_rules));
            }
        }
    };
}

async fn update_filters_deprecated(
    filter: &mut fnet_filter_deprecated::FilterProxy,
    config: FilterConfig,
) -> Result<(), anyhow::Error> {
    let FilterConfig { rules, nat_rules, rdr_rules } = config;

    if !rules.is_empty() {
        let rules = netfilter::parser_deprecated::parse_str_to_rules(&rules.join(""))
            .context("error parsing filter rules")?;
        cas_filter_rules!(filter, get_rules, update_rules, rules, FilterUpdateRulesError);
    }

    if !nat_rules.is_empty() {
        let nat_rules = netfilter::parser_deprecated::parse_str_to_nat_rules(&nat_rules.join(""))
            .context("error parsing NAT rules")?;
        cas_filter_rules!(
            filter,
            get_nat_rules,
            update_nat_rules,
            nat_rules,
            FilterUpdateNatRulesError
        );
    }

    if !rdr_rules.is_empty() {
        let rdr_rules = netfilter::parser_deprecated::parse_str_to_rdr_rules(&rdr_rules.join(""))
            .context("error parsing RDR rules")?;
        // TODO(https://fxbug.dev/42147284): Change this to cas_filter_rules once
        // update is supported.
        no_update_filter_rules!(
            filter,
            get_rdr_rules,
            update_rdr_rules,
            rdr_rules,
            FilterUpdateRdrRulesError
        );
    }

    Ok(())
}

#[derive(Debug)]
struct MasqueradeCounter(NonZeroU64);

impl MasqueradeCounter {
    fn new() -> Self {
        Self(NonZeroU64::new(1).unwrap())
    }

    fn increment(&mut self) {
        *self = Self(self.0.checked_add(1).expect("integer_overflow on u64"));
    }

    fn decrement(&self) -> Option<Self> {
        NonZeroU64::new(self.0.get() - 1).map(Self)
    }
}

#[derive(Debug, Default)]
pub(super) struct FilterEnabledState {
    interface_types: HashSet<InterfaceType>,
    // A map of interface ID to the number of active masquerade configurations
    // applied on that interface.
    masquerade_enabled_interface_ids: HashMap<InterfaceId, MasqueradeCounter>,
    // Indexed by interface id and stores `RuleId`s inserted for that interface.
    // All rules for an interface should be removed upon interface removal.
    // Vec will always be empty when using filter.deprecated.
    //
    // Note: Masquerade rules are not held here. Filtering on an interface can
    // only be disabled when there are no masquerade configurations remaining
    // (i.e. absence of an `InterfaceId` from `masquerade_enabled_interface_ids`
    // is proof that there are no installed Masquerade Rules on the interface).
    currently_enabled_interfaces: HashMap<InterfaceId, Vec<RuleId>>,
}

impl FilterEnabledState {
    pub(super) fn new(interface_types: HashSet<InterfaceType>) -> Self {
        Self { interface_types, ..Default::default() }
    }

    /// Updates the filter state for the provided `interface_id` using either
    /// fuchsia.net.filter.deprecated or fuchsia.net.filter.
    ///
    /// `interface_type`: The type of the given interface. If the type cannot be
    /// determined, this will be None, and `FilterEnabledState::interface_types`
    /// will be ignored.
    pub(super) async fn maybe_update(
        &mut self,
        interface_type: Option<InterfaceType>,
        interface_id: InterfaceId,
        filter: &mut FilterControl,
    ) -> Result<(), anyhow::Error> {
        match filter {
            FilterControl::Deprecated(proxy) => self
                .maybe_update_deprecated(interface_type, interface_id, proxy)
                .await
                .map_err(|e| anyhow::anyhow!("{e:?}")),
            FilterControl::Current(filter_state) => self
                .maybe_update_current(interface_type, interface_id, filter_state)
                .await
                .map_err(|e| anyhow::anyhow!("{e:?}")),
        }
    }

    /// Clears tracking and masquerade counts for a removed interface.
    /// Netstack automatically destroys the rules on interface removal.
    pub(super) fn remove_interface(&mut self, interface_id: InterfaceId) {
        let _removed_rules: Option<Vec<RuleId>> =
            self.currently_enabled_interfaces.remove(&interface_id);
        let _removed_count: Option<MasqueradeCounter> =
            self.masquerade_enabled_interface_ids.remove(&interface_id);
    }

    pub(super) async fn maybe_update_deprecated<
        Filter: fnet_filter_deprecated::FilterProxyInterface,
    >(
        &mut self,
        interface_type: Option<InterfaceType>,
        interface_id: InterfaceId,
        filter: &Filter,
    ) -> Result<(), fnet_filter_deprecated::EnableDisableInterfaceError> {
        let should_be_enabled = self.should_enable(interface_type, interface_id);
        let is_enabled = self.currently_enabled_interfaces.entry(interface_id);

        match (should_be_enabled, is_enabled) {
            (true, Entry::Vacant(entry)) => {
                if let Err(e) = filter
                    .enable_interface(interface_id.get())
                    .await
                    .unwrap_or_else(|err| exit_with_fidl_error(err))
                {
                    warn!("failed to enable interface {interface_id}: {e:?}");
                    return Err(e);
                }
                let _ = entry.insert(vec![]);
            }
            (false, Entry::Occupied(entry)) => {
                if let Err(e) = filter
                    .disable_interface(interface_id.get())
                    .await
                    .unwrap_or_else(|err| exit_with_fidl_error(err))
                {
                    warn!("failed to disable interface {interface_id}: {e:?}");
                    return Err(e);
                }
                let _ = entry.remove();
            }
            (true, Entry::Occupied(_)) | (false, Entry::Vacant(_)) => {
                // Do nothing. The interface's current state aligns with
                // whether it is present in the map.
            }
        }
        Ok(())
    }

    pub(super) async fn maybe_update_current(
        &mut self,
        interface_type: Option<InterfaceType>,
        interface_id: InterfaceId,
        filter: &mut FilterState,
    ) -> Result<(), FilterError> {
        let should_be_enabled = self.should_enable(interface_type, interface_id);
        let is_enabled = self.currently_enabled_interfaces.entry(interface_id);

        match (should_be_enabled, is_enabled) {
            (true, Entry::Vacant(entry)) => {
                let FilterState {
                    controller,
                    uninstalled_ip_routines,
                    installed_ip_routines,
                    current_installed_rule_index,
                    masquerade: _,
                } = filter;
                let rules = generate_updated_filter_rules(
                    uninstalled_ip_routines,
                    installed_ip_routines,
                    interface_id,
                    *current_installed_rule_index,
                );

                if !rules.is_empty() {
                    let rule_changes = rules
                        .clone()
                        .into_iter()
                        .map(|rule| Change::Create(Resource::Rule(rule)))
                        .collect();
                    controller.push_changes(rule_changes).await.map_err(FilterError::Push)?;
                    controller.commit().await.map_err(FilterError::Commit)?;
                    info!(
                        "new filter rules for iface with id {interface_id:?} \
                                have been committed successfully"
                    );
                    // Increment the current rule index only on success since
                    // `commit` will either apply changes in entirety, or none
                    // at all.
                    *current_installed_rule_index = current_installed_rule_index.wrapping_add(1);
                }

                // Get the `RuleId`s from the inserted `Rule`s so that they can be
                // removed if the interface is disabled.
                let rule_ids: Vec<_> = rules.into_iter().map(|rule| rule.id).collect();
                let _ = entry.insert(rule_ids);
            }
            (false, Entry::Occupied(entry)) => {
                let FilterState { controller, .. } = filter;
                let rule_changes: Vec<_> = entry
                    .remove()
                    .into_iter()
                    .map(|rule_id| Change::Remove(ResourceId::Rule(rule_id)))
                    .collect();

                if !rule_changes.is_empty() {
                    controller.push_changes(rule_changes).await.map_err(FilterError::Push)?;
                    controller.commit().await.map_err(FilterError::Commit)?;
                    info!(
                        "removal of filter rules for iface with id {interface_id:?} \
                                have been committed successfully"
                    );
                }
            }
            (true, Entry::Occupied(_)) | (false, Entry::Vacant(_)) => {
                // Do nothing. The interface's current state aligns with
                // whether it is present in the map.
            }
        }

        Ok(())
    }

    /// Determines whether a given `interface_id` should be enabled.
    ///
    /// `interface_type`: The type of the given interface. If the type cannot be
    /// determined, this will be None, and `FilterEnabledState::interface_types`
    /// will be ignored.
    fn should_enable(
        &self,
        interface_type: Option<InterfaceType>,
        interface_id: InterfaceId,
    ) -> bool {
        interface_type
            .as_ref()
            .map(|ty| match ty {
                InterfaceType::WlanClient
                | InterfaceType::Ethernet
                | InterfaceType::Blackhole
                | InterfaceType::Lowpan => self.interface_types.contains(ty),
                // An AP device can be filtered by specifying AP or WLAN.
                InterfaceType::WlanAp => {
                    self.interface_types.contains(ty)
                        | self.interface_types.contains(&InterfaceType::WlanClient)
                }
            })
            .unwrap_or(false)
            || self.masquerade_enabled_interface_ids.contains_key(&interface_id)
    }

    pub(crate) fn increment_masquerade_count_on_interface(&mut self, interface_id: InterfaceId) {
        match self.masquerade_enabled_interface_ids.entry(interface_id) {
            Entry::Vacant(entry) => {
                let _new_count = entry.insert(MasqueradeCounter::new());
            }
            Entry::Occupied(mut entry) => entry.get_mut().increment(),
        }
    }

    pub(crate) fn decrement_masquerade_count_on_interface(&mut self, interface_id: InterfaceId) {
        match self.masquerade_enabled_interface_ids.entry(interface_id) {
            Entry::Vacant(_) => panic!(
                "asked to decrement the masquerade count for a non-configured interface: {}",
                interface_id
            ),
            Entry::Occupied(mut entry) => match entry.get().decrement() {
                // Subtraction made the count 0; remove it.
                None => {
                    let _old_count = entry.remove();
                }
                Some(count) => {
                    let _old_count = entry.insert(count);
                }
            },
        }
    }
}

/// Repeatedly attempts to update the NAT rules using `fuchsia.net.filter.deprecated`.
///
/// The update will be attempted up to `FILTER_CAS_RETRY_MAX` times.
async fn update_nat_rules_deprecated(
    filter: &mut fnet_filter_deprecated::FilterProxy,
    update_fn: impl Fn(&mut Vec<fnet_filter_deprecated::Nat>) -> Result<(), fnet_masquerade::Error>,
) -> Result<(), fnet_masquerade::Error> {
    for _ in 0..FILTER_CAS_RETRY_MAX {
        let (mut rules, generation) =
            filter.get_nat_rules().await.expect("call to GetNatRules failed");
        update_fn(&mut rules)?;

        match filter
            .update_nat_rules(&rules, generation)
            .await
            .expect("call to UpdateNatRules failed")
        {
            Ok(()) => return Ok(()),
            Err(fnet_filter_deprecated::FilterUpdateNatRulesError::GenerationMismatch) => {
                // We need to try again.
                fuchsia_async::Timer::new(
                    zx::MonotonicDuration::from_millis(
                        crate::filter::FILTER_CAS_RETRY_INTERVAL_MILLIS,
                    )
                    .after_now(),
                )
                .await;
            }
            Err(fnet_filter_deprecated::FilterUpdateNatRulesError::BadRule) => {
                // This can sometimes be triggered if the NIC is deleted before
                // the call to `update_nat_rules`.
                error!(
                    "Generated Nat rule was invalid. Perhaps the requested \
                     NIC has been removed {rules:?}"
                );
                // There is no point in retrying in this case.
                return Err(fnet_masquerade::Error::BadRule);
            }
        }
    }

    error!("Failed to update Nat rules");
    Err(fnet_masquerade::Error::RetryExceeded)
}

// Attempts to add a new masquerade NAT rule using `fuchsia.net.filter.deprecated`.
pub(crate) async fn add_masquerade_rule_deprecated(
    filter: &mut fnet_filter_deprecated::FilterProxy,
    rule: fnet_filter_deprecated::Nat,
) -> Result<(), fnet_masquerade::Error> {
    update_nat_rules_deprecated(filter, |rules| {
        if rules.iter().any(|existing_rule| existing_rule == &rule) {
            Err(fnet_masquerade::Error::AlreadyExists)
        } else {
            rules.push(rule.clone());
            Ok(())
        }
    })
    .await
}

// Attempts to remove an existing masquerade NAT rule using `fuchsia.net.filter.deprecated`.
pub(crate) async fn remove_masquerade_rule_deprecated(
    filter: &mut fnet_filter_deprecated::FilterProxy,
    rule: fnet_filter_deprecated::Nat,
) -> Result<(), fnet_masquerade::Error> {
    update_nat_rules_deprecated(filter, |rules| {
        rules.retain(|existing_rule| existing_rule != &rule);
        Ok(())
    })
    .await
}

// Attempts to add a new masquerade NAT rule using `fuchsia.net.filter`.
pub(crate) async fn add_masquerade_rule_current(
    filter: &mut FilterState,
    matchers: Matchers,
) -> Result<RuleId, FilterError> {
    let MasqueradeState { routine_id, next_rule_index } = &mut filter.masquerade;
    let rule_id = RuleId { routine: routine_id.clone(), index: *next_rule_index };
    let rule_changes = vec![Change::Create(Resource::Rule(Rule {
        id: rule_id.clone(),
        matchers: matchers,
        action: Action::Masquerade { src_port: None },
    }))];
    filter.controller.push_changes(rule_changes).await.map_err(FilterError::Push)?;
    filter.controller.commit().await.map_err(FilterError::Commit)?;
    *next_rule_index += 1;
    Ok(rule_id)
}

// Attempts to remove an existing masquerade NAT rule using `fuchsia.net.filter`.
pub(crate) async fn remove_masquerade_rule_current(
    filter: &mut FilterState,
    rule: &RuleId,
) -> Result<(), FilterError> {
    let rule_changes = vec![Change::Remove(ResourceId::Rule(rule.clone()))];
    filter.controller.push_changes(rule_changes).await.map_err(FilterError::Push)?;
    filter.controller.commit().await.map_err(FilterError::Commit)
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use crate::DeviceClass;
    use crate::interface::DeviceInfoRef;

    use super::*;

    const INTERFACE_ID: InterfaceId = InterfaceId::new(1).unwrap();
    const LOCAL_INGRESS: &str = "local_ingress";
    const UNINSTALLED_LOCAL_INGRESS: &str = "local_ingress_uninstalled";
    const LOCAL_EGRESS: &str = "local_egress";
    const UNINSTALLED_LOCAL_EGRESS: &str = "local_egress_uninstalled";
    const MASQUERADE: &str = "egress_masquerade";

    fn get_foundational_changes() -> Vec<Change> {
        let mut changes = vec![Change::Create(Resource::Namespace(Namespace {
            id: namespace_id(),
            domain: Domain::AllIp,
        }))];

        let local_ingress = (LOCAL_INGRESS, UNINSTALLED_LOCAL_INGRESS, IpHook::LocalIngress);
        let local_egress = (LOCAL_EGRESS, UNINSTALLED_LOCAL_EGRESS, IpHook::LocalEgress);

        let routine_changes = vec![local_ingress, local_egress]
            .into_iter()
            .map(|(installed_name, uninstalled_name, hook)| {
                vec![
                    Routine {
                        id: RoutineId {
                            namespace: namespace_id(),
                            name: String::from(uninstalled_name),
                        },
                        routine_type: RoutineType::Ip(None),
                    },
                    Routine {
                        id: RoutineId {
                            namespace: namespace_id(),
                            name: String::from(installed_name),
                        },
                        routine_type: RoutineType::Ip(Some(InstalledIpRoutine {
                            hook,
                            priority: 0i32,
                        })),
                    },
                ]
            })
            .flatten()
            .chain([Routine {
                id: RoutineId { namespace: namespace_id(), name: String::from(MASQUERADE) },
                routine_type: RoutineType::Nat(Some(InstalledNatRoutine {
                    hook: NatHook::Egress,
                    priority: 0i32,
                })),
            }])
            .map(|routine| Change::Create(Resource::Routine(routine)));
        changes.extend(routine_changes);

        changes
    }

    fn create_rule(routine: RoutineId, index: u32, action: Action) -> Rule {
        Rule { id: RuleId { routine, index }, matchers: Matchers::default(), action }
    }

    fn create_routine_id(name: &str) -> RoutineId {
        RoutineId { namespace: namespace_id(), name: String::from(name) }
    }

    fn create_filter_routines(
        namespace: NamespaceId,
        local_ingress: &str,
        local_egress: &str,
    ) -> netfilter::parser::FilterRoutines {
        netfilter::parser::FilterRoutines {
            local_ingress: Some(RoutineId {
                namespace: namespace.clone(),
                name: local_ingress.to_owned(),
            }),
            local_egress: Some(RoutineId { namespace, name: local_egress.to_owned() }),
        }
    }

    // This test only checks for `Ok` cases. The only possible failures for the function under
    // test are related to Rule parsing, which the netfilter library already tests.
    #[test_case(vec![], vec![]; "no_rules")]
    #[test_case(
        vec!["pass in;"],
        vec![create_rule(
                create_routine_id(UNINSTALLED_LOCAL_INGRESS),
                0,
                Action::Accept,
            )]; "ingress_accept")]
    #[test_case(
        vec!["drop out;"],
        vec![create_rule(
                create_routine_id(UNINSTALLED_LOCAL_EGRESS),
                0,
                Action::Drop,
            )]; "egress_drop")]
    #[test_case(
        vec!["pass in; drop out;"],
        vec![create_rule(
                create_routine_id(UNINSTALLED_LOCAL_INGRESS),
                0,
                Action::Accept),
            create_rule(
                create_routine_id(UNINSTALLED_LOCAL_EGRESS),
                1,
                Action::Drop,
            )]; "ingress_accept_egress_drop")]
    fn test_initial_filter_changes(rules_input: Vec<&str>, expected_rules: Vec<Rule>) {
        let namespace = namespace_id();
        let installed_filter_routines =
            create_filter_routines(namespace.clone(), LOCAL_INGRESS, LOCAL_EGRESS);
        let uninstalled_filter_routines =
            create_filter_routines(namespace, UNINSTALLED_LOCAL_INGRESS, UNINSTALLED_LOCAL_EGRESS);

        let (changes, _num_installed_rules) = generate_initial_filter_changes(
            &uninstalled_filter_routines,
            &installed_filter_routines,
            &masquerade_routine(),
            FilterConfig {
                rules: rules_input.into_iter().map(|rule| rule.to_owned()).collect(),
                nat_rules: vec![],
                rdr_rules: vec![],
            },
            &HashSet::new(),
            0,
        )
        .expect("rules should be formatted correctly");

        let mut expected_changes = get_foundational_changes();
        let expected_rule_changes =
            expected_rules.into_iter().map(|rule| Change::Create(Resource::Rule(rule)));
        expected_changes.extend(expected_rule_changes);

        assert_eq!(changes, expected_changes);
    }

    #[test]
    fn test_generate_updated_filter_rules() {
        let namespace = namespace_id();
        let installed_filter_routines =
            create_filter_routines(namespace.clone(), LOCAL_INGRESS, LOCAL_EGRESS);
        let uninstalled_filter_routines =
            create_filter_routines(namespace, UNINSTALLED_LOCAL_INGRESS, UNINSTALLED_LOCAL_EGRESS);

        let rules = generate_updated_filter_rules(
            &uninstalled_filter_routines,
            &installed_filter_routines,
            INTERFACE_ID,
            0,
        );

        let local_ingress = (
            installed_filter_routines.local_ingress.unwrap(),
            uninstalled_filter_routines.local_ingress.unwrap().name,
            IpHook::LocalIngress,
        );
        let local_egress = (
            installed_filter_routines.local_egress.unwrap(),
            uninstalled_filter_routines.local_egress.unwrap().name,
            IpHook::LocalEgress,
        );
        let expected_rules: Vec<_> = vec![local_ingress, local_egress]
            .into_iter()
            .map(|(installed_routine, uninstalled_routine_name, hook)| {
                create_interface_matching_jump_rule(
                    installed_routine,
                    0,
                    INTERFACE_ID,
                    hook,
                    &uninstalled_routine_name,
                )
            })
            .collect();

        assert_eq!(rules, expected_rules);
    }

    #[test]
    fn test_generate_static_port_class_filter_rules() {
        let namespace = namespace_id();
        let installed_filter_routines =
            create_filter_routines(namespace.clone(), LOCAL_INGRESS, LOCAL_EGRESS);
        let uninstalled_filter_routines =
            create_filter_routines(namespace, UNINSTALLED_LOCAL_INGRESS, UNINSTALLED_LOCAL_EGRESS);

        let rules = generate_static_port_class_filter_rules(
            &uninstalled_filter_routines,
            &installed_filter_routines,
            fnet_interfaces_ext::PortClass::Lowpan,
            0,
        );

        let local_ingress = (
            installed_filter_routines.local_ingress.unwrap(),
            uninstalled_filter_routines.local_ingress.unwrap().name,
            IpHook::LocalIngress,
        );
        let local_egress = (
            installed_filter_routines.local_egress.unwrap(),
            uninstalled_filter_routines.local_egress.unwrap().name,
            IpHook::LocalEgress,
        );
        let expected_rules: Vec<_> = vec![local_ingress, local_egress]
            .into_iter()
            .map(|(installed_routine, uninstalled_routine_name, hook)| {
                create_port_class_matching_jump_rule(
                    installed_routine,
                    0,
                    fnet_interfaces_ext::PortClass::Lowpan,
                    hook,
                    &uninstalled_routine_name,
                )
            })
            .collect();

        assert_eq!(rules, expected_rules);
    }

    #[test]
    fn test_initial_filter_changes_with_lowpan() {
        let namespace = namespace_id();
        let installed_filter_routines =
            create_filter_routines(namespace.clone(), LOCAL_INGRESS, LOCAL_EGRESS);
        let uninstalled_filter_routines =
            create_filter_routines(namespace, UNINSTALLED_LOCAL_INGRESS, UNINSTALLED_LOCAL_EGRESS);

        let (changes, num_installed_rules) = generate_initial_filter_changes(
            &uninstalled_filter_routines,
            &installed_filter_routines,
            &masquerade_routine(),
            FilterConfig { rules: vec![], nat_rules: vec![], rdr_rules: vec![] },
            &[InterfaceType::Lowpan].into_iter().collect(),
            0,
        )
        .expect("rules should be formatted correctly");

        let mut expected_changes = get_foundational_changes();

        let lowpan_rules = generate_static_port_class_filter_rules(
            &uninstalled_filter_routines,
            &installed_filter_routines,
            fnet_interfaces_ext::PortClass::Lowpan,
            0,
        );
        expected_changes
            .extend(lowpan_rules.into_iter().map(|rule| Change::Create(Resource::Rule(rule))));

        assert_eq!(changes, expected_changes);
        assert_eq!(num_installed_rules, 1);
    }

    #[test]
    fn test_should_enable_filter() {
        let types_empty: HashSet<InterfaceType> = [].iter().cloned().collect();
        let types_ethernet: HashSet<InterfaceType> =
            [InterfaceType::Ethernet].iter().cloned().collect();
        let types_wlan: HashSet<InterfaceType> =
            [InterfaceType::WlanClient].iter().cloned().collect();
        let types_ap: HashSet<InterfaceType> = [InterfaceType::WlanAp].iter().cloned().collect();

        let id = InterfaceId::new(10).unwrap();

        let make_info = |device_class| DeviceInfoRef {
            device_class,
            mac: &fidl_fuchsia_net_ext::MacAddress { octets: [0x1, 0x1, 0x1, 0x1, 0x1, 0x1] },
            topological_path: "",
        };

        let wlan_info = make_info(DeviceClass::WlanClient);
        let wlan_ap_info = make_info(DeviceClass::WlanAp);
        let ethernet_info = make_info(DeviceClass::Ethernet);

        let mut fes = FilterEnabledState::new(types_empty.clone());
        assert_eq!(fes.should_enable(Some(wlan_info.interface_type()), id), false);
        assert_eq!(fes.should_enable(Some(wlan_ap_info.interface_type()), id), false);
        assert_eq!(fes.should_enable(Some(ethernet_info.interface_type()), id), false);

        fes.increment_masquerade_count_on_interface(id);
        assert_eq!(fes.should_enable(Some(ethernet_info.interface_type()), id), true);

        let mut fes = FilterEnabledState::new(types_ethernet);
        assert_eq!(fes.should_enable(Some(wlan_info.interface_type()), id), false);
        assert_eq!(fes.should_enable(Some(wlan_ap_info.interface_type()), id), false);
        assert_eq!(fes.should_enable(Some(ethernet_info.interface_type()), id), true);

        fes.increment_masquerade_count_on_interface(id);
        assert_eq!(fes.should_enable(Some(wlan_info.interface_type()), id), true);

        let mut fes = FilterEnabledState::new(types_wlan);
        assert_eq!(fes.should_enable(Some(wlan_info.interface_type()), id), true);
        assert_eq!(fes.should_enable(Some(wlan_ap_info.interface_type()), id), true);
        assert_eq!(fes.should_enable(Some(ethernet_info.interface_type()), id), false);

        fes.increment_masquerade_count_on_interface(id);
        assert_eq!(fes.should_enable(Some(ethernet_info.interface_type()), id), true);

        let mut fes = FilterEnabledState::new(types_ap);
        assert_eq!(fes.should_enable(Some(wlan_info.interface_type()), id), false);
        assert_eq!(fes.should_enable(Some(wlan_ap_info.interface_type()), id), true);
        assert_eq!(fes.should_enable(Some(ethernet_info.interface_type()), id), false);

        fes.increment_masquerade_count_on_interface(id);
        assert_eq!(fes.should_enable(Some(wlan_info.interface_type()), id), true);
        assert_eq!(fes.should_enable(Some(ethernet_info.interface_type()), id), true);

        // Verify that the count can be decremented while keeping filtering enabled.
        let mut fes = FilterEnabledState::new(types_empty);
        for _ in 0..3 {
            fes.increment_masquerade_count_on_interface(id);
        }
        for expect_enabled in [true, true, false] {
            fes.decrement_masquerade_count_on_interface(id);
            assert_eq!(fes.should_enable(Some(wlan_info.interface_type()), id), expect_enabled);
            assert_eq!(fes.should_enable(Some(wlan_ap_info.interface_type()), id), expect_enabled);
            assert_eq!(fes.should_enable(Some(ethernet_info.interface_type()), id), expect_enabled);
        }
    }

    #[fuchsia::test]
    async fn test_update_filters_current_large_batch() {
        use futures::StreamExt as _;

        let (control_client, control_server) =
            fidl::endpoints::create_endpoints::<fnet_filter::ControlMarker>();
        let client_fut = FilterControl::new(None, Some(control_client.into_proxy()));
        let mut control_stream = control_server.into_stream();
        let control_server_fut = async move {
            match control_stream
                .next()
                .await
                .expect("stream shouldn't close")
                .expect("stream shouldn't have an error")
            {
                fnet_filter::ControlRequest::OpenController { id, request, control_handle: _ } => {
                    let (request_stream, control_handle) = request.into_stream_and_control_handle();
                    control_handle.send_on_id_assigned(id.as_str()).expect("failed to respond");
                    request_stream
                }
                _ => panic!("unexpected request"),
            }
        };
        let (filter_control, mut server_request_stream) =
            futures::join!(client_fut, control_server_fut);
        let mut filter_control = filter_control.expect("failed to create filter control");

        let config = FilterConfig {
            rules: std::iter::repeat("pass in;".to_string()).take(50).collect(),
            nat_rules: vec![],
            rdr_rules: vec![],
        };

        let server_fut = async move {
            let mut push_changes_count = 0;
            while let Some(req) = server_request_stream.next().await {
                match req.expect("stream shouldn't have an error") {
                    fnet_filter::NamespaceControllerRequest::PushChanges { changes, responder } => {
                        assert!(
                            changes.len() <= usize::from(fnet_filter::MAX_BATCH_SIZE),
                            "batch size {} exceeds MAX_BATCH_SIZE",
                            changes.len()
                        );
                        push_changes_count += 1;
                        responder
                            .send(fnet_filter::ChangeValidationResult::Ok(fnet_filter::Empty))
                            .expect("failed to respond");
                    }
                    fnet_filter::NamespaceControllerRequest::Commit { payload: _, responder } => {
                        responder
                            .send(fnet_filter::CommitResult::Ok(fnet_filter::Empty))
                            .expect("failed to respond");
                        break;
                    }
                    _ => panic!("unexpected request"),
                }
            }
            push_changes_count
        };

        let filter_enabled_state = FilterEnabledState::default();
        let (client_res, push_changes_count) = futures::join!(
            filter_control.update_filters(config, &filter_enabled_state),
            server_fut
        );

        client_res.expect("update_filters should succeed");
        assert_eq!(push_changes_count, 2);
    }
}
