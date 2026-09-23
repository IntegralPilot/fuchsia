// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A module for managing policy based routing (PBR) rules.
//! Supports the following NETLINK_ROUTE requests: RTM_GETRULE, RTM_SETRULE, &
//! RTM_DELRULE.

use std::collections::BTreeMap;
use std::time::Duration;

use assert_matches::assert_matches;
use derivative::Derivative;
use fidl_fuchsia_net_routes_admin::{GrantForRouteTableAuthorization, RuleSetError};
use fidl_fuchsia_net_routes_ext::admin::FidlRouteAdminIpExt;
use linux_uapi::{
    rt_class_t_RT_TABLE_DEFAULT, rt_class_t_RT_TABLE_LOCAL, rt_class_t_RT_TABLE_MAIN,
};
use net_types::ip::{GenericOverIp, Ip, IpVersion, IpVersionMarker, Ipv4, Ipv6};
use netlink_packet_core::{NLM_F_MULTIPART, NetlinkMessage};
use netlink_packet_route::rule::{RuleAction, RuleAttribute, RuleMessage};
use netlink_packet_route::{AddressFamily, RouteNetlinkMessage};

use fidl::endpoints::ProtocolMarker;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_routes_ext::rules::{FidlRuleAdminIpExt, FidlRuleIpExt};
use fidl_fuchsia_net_routes_ext::{self as fnet_routes_ext, FidlRouteIpExt};

use crate::client::InternalClient;
use crate::logging::log_warn;
use crate::messaging::Sender;
use crate::netlink_packet::errno::Errno;
use crate::protocol_family::ProtocolFamily;
use crate::protocol_family::route::NetlinkRoute;
use crate::route_tables::{
    ManagedRouteTable, NetlinkRouteTableIndex, RouteTable, RouteTableMap, TableNeedsCleanup,
    UnmanagedTable,
};

mod conversions;

// The priorities of the default rules installed on Linux.
const LINUX_DEFAULT_LOOKUP_LOCAL_PRIORITY: u32 = 0;
const LINUX_DEFAULT_LOOKUP_MAIN_PRIORITY: u32 = 32766;
const LINUX_DEFAULT_LOOKUP_DEFAULT_PRIORITY: u32 = 32767;

// This is a bit arbitrary, but is set so that the leading bit of the resulting RuleIndex is 1 once
// this is shifted left 16 bits in order to be included.
const NETLINK_RULE_SET_PRIORITY: fnet_routes_ext::rules::RuleSetPriority =
    fnet_routes_ext::rules::RuleSetPriority::new((1u16 << 15) as u32);

type RulePriority = u32;

/// Witness that a [`RuleMessage`] has a [`RulePriority`].
#[derive(Clone)]
struct RuleMessageWithPriority {
    rule_message: RuleMessage,
    priority: u32,
}

impl TryFrom<RuleMessage> for RuleMessageWithPriority {
    type Error = RuleMessage;

    fn try_from(rule_message: RuleMessage) -> Result<Self, Self::Error> {
        if let Some(priority) = get_priority(&rule_message) {
            Ok(RuleMessageWithPriority { rule_message, priority })
        } else {
            Err(rule_message)
        }
    }
}

/// Helper to retrieve the `Priority` NLA from a [`RuleMessage`].
fn get_priority(RuleMessage { header: _, attributes, .. }: &RuleMessage) -> Option<RulePriority> {
    attributes.iter().find_map(|nla| match nla {
        RuleAttribute::Priority(priority) => Some(*priority),
        _ => None,
    })
}

/// Returns true if the two rules are equal, ignoring nla order.
fn rules_are_equal(
    RuleMessage { header: header1, attributes: nlas1, .. }: &RuleMessage,
    RuleMessage { header: header2, attributes: nlas2, .. }: &RuleMessage,
) -> bool {
    if header1 != header2 || nlas1.len() != nlas2.len() {
        return false;
    }
    nlas1.iter().all(|nla| nlas2.contains(nla))
}

/// Returns true if the specified pattern is valid.
fn is_valid_del_pattern(RuleMessage { header, attributes, .. }: &RuleMessage) -> bool {
    // Either an action, or an NLA must be specified.
    attributes.len() != 0 || header.action != RuleAction::Unspec
}

/// Returns true if the given rule matches the given deletion pattern.
fn rule_matches_del_pattern(rule: &RuleMessage, del_pattern: &RuleMessage) -> bool {
    let RuleMessage { header: rule_header, attributes: rule_nlas, .. } = rule;
    let RuleMessage { header: pattern_header, attributes: pattern_nlas, .. } = del_pattern;

    // If the pattern specifies an action, it must match the rule's action.
    if pattern_header.action != RuleAction::Unspec && rule_header.action != pattern_header.action {
        return false;
    }

    // Any NLA specified by the pattern must be present in the rule, with the
    // same value.
    for pattern_nla in pattern_nlas {
        if !rule_nlas.iter().any(|rule_nla| rule_nla == pattern_nla) {
            return false;
        }
    }
    true
}

/// Converts the [`RuleMessage`] into a RtnlMessage::NewRule [`NetlinkMessage`].
fn to_nlm_new_rule(
    rule: RuleMessage,
    sequence_number: u32,
    dump: bool,
) -> NetlinkMessage<RouteNetlinkMessage> {
    let mut msg: NetlinkMessage<RouteNetlinkMessage> = RouteNetlinkMessage::NewRule(rule).into();
    msg.header.sequence_number = sequence_number;
    if dump {
        msg.header.flags = NLM_F_MULTIPART;
    }
    msg.finalize();
    msg
}

/// Maps from the lower-order bits of a rule's [`fnet_routes_ext::RuleIndex`] to the rule stored
/// at that index.
/// (The higher-order bits will come from the [`RulePriority`].)
#[derive(Default, Debug)]
struct IndexedRules {
    rules: BTreeMap<u16, RuleMessage>,
}

impl IndexedRules {
    fn add_rule(&mut self, rule: RuleMessage) -> Result<u16, AddRuleError> {
        if self.rules.values().any(|existing_rule| rules_are_equal(existing_rule, &rule)) {
            return Err(AddRuleError::AlreadyExists);
        }

        let last_allocated_index = self.rules.keys().next_back();

        let next_available_index = match last_allocated_index {
            None => 0u16,
            Some(index) => index.checked_add(1).ok_or_else(|| {
                crate::logging::log_error!("Could not add rule due to exhausting u16 indices");
                AddRuleError::IndicesExhausted
            })?,
        };

        assert_matches!(self.rules.insert(next_available_index, rule), None);
        Ok(next_available_index)
    }

    fn remove_first_matching(&mut self, pattern: &RuleMessage) -> Option<(u16, RuleMessage)> {
        let index = self
            .rules
            .iter()
            .find_map(|(index, rule)| rule_matches_del_pattern(rule, pattern).then_some(*index))?;
        let rule = self.rules.remove(&index)?;
        Some((index, rule))
    }

    fn remove(&mut self, index: u16) -> Option<RuleMessage> {
        self.rules.remove(&index)
    }

    fn iter(&self) -> impl Iterator<Item = &RuleMessage> {
        self.rules.values()
    }

    fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

pub(crate) trait IpExt:
    Ip + FidlRuleAdminIpExt + FidlRuleIpExt + FidlRouteAdminIpExt + FidlRouteIpExt
{
}
impl<I: Ip + FidlRuleAdminIpExt + FidlRuleIpExt + FidlRouteAdminIpExt + FidlRouteIpExt> IpExt
    for I
{
}

/// Holds an IP-versioned table of PBR rules.
#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub(crate) struct RuleTable<I: IpExt> {
    /// The rules held by this rule table.
    ///
    /// The [`BTreeMap`] ensures that the rules are sorted by their
    /// [`RulePriority`], while the held [`IndexedRules`] ensures the rules at a given
    /// [`RulePriority`] are held in insertion order (new rules are pushed onto
    /// the back). This gives the rule table a consistent ordering based first
    /// on priority, and then by age.
    rules: BTreeMap<RulePriority, IndexedRules>,
    _ip_version_marker: IpVersionMarker<I>,
}

impl<I: IpExt> RuleTable<I> {
    /// The default rules present on Linux.
    /// * [V4] 0:        from all lookup local
    /// * [V4] 32766:    from all lookup main
    /// * [V4] 32767:    from all lookup default
    /// * [V6] 0:        from all lookup local
    /// * [V6] 32766:    from all lookup main
    fn default_rules() -> impl Iterator<Item = RuleMessage> {
        fn build_lookup_rule<I: IpExt>(priority: RulePriority, table: u8) -> RuleMessage {
            let mut rule = RuleMessage::default();
            rule.header.family = match I::VERSION {
                IpVersion::V4 => AddressFamily::Inet,
                IpVersion::V6 => AddressFamily::Inet6,
            };
            rule.header.table = table;
            rule.header.action = RuleAction::ToTable;
            rule.attributes.push(RuleAttribute::Priority(priority));
            rule
        }

        match I::VERSION {
            IpVersion::V4 => itertools::Either::Left(
                [
                    build_lookup_rule::<Ipv4>(
                        LINUX_DEFAULT_LOOKUP_LOCAL_PRIORITY,
                        rt_class_t_RT_TABLE_LOCAL as u8,
                    ),
                    build_lookup_rule::<Ipv4>(
                        LINUX_DEFAULT_LOOKUP_MAIN_PRIORITY,
                        rt_class_t_RT_TABLE_MAIN as u8,
                    ),
                    build_lookup_rule::<Ipv4>(
                        LINUX_DEFAULT_LOOKUP_DEFAULT_PRIORITY,
                        rt_class_t_RT_TABLE_DEFAULT as u8,
                    ),
                ]
                .into_iter(),
            ),
            IpVersion::V6 => itertools::Either::Right(
                [
                    build_lookup_rule::<Ipv6>(
                        LINUX_DEFAULT_LOOKUP_LOCAL_PRIORITY,
                        rt_class_t_RT_TABLE_LOCAL as u8,
                    ),
                    build_lookup_rule::<Ipv6>(
                        LINUX_DEFAULT_LOOKUP_MAIN_PRIORITY,
                        rt_class_t_RT_TABLE_MAIN as u8,
                    ),
                ]
                .into_iter(),
            ),
        }
    }

    /// Constructs an empty RuleTable.
    pub(crate) fn new() -> RuleTable<I> {
        RuleTable { rules: BTreeMap::default(), _ip_version_marker: I::VERSION_MARKER }
    }

    /// Constructs a RuleTable prepopulated with the default rules present on
    /// Linux.
    #[cfg(test)]
    pub(crate) fn new_with_defaults() -> RuleTable<I> {
        let mut table =
            RuleTable { rules: BTreeMap::default(), _ip_version_marker: I::VERSION_MARKER };
        for rule in Self::default_rules() {
            let _index = table
                .add_rule(table.apply_default_priority_if_needed(rule))
                .expect("should not fail to add a default rule");
        }

        table
    }

    fn apply_default_priority_if_needed(&self, rule: RuleMessage) -> RuleMessageWithPriority {
        match rule.try_into() {
            Ok(rule) => rule,
            Err(mut rule) => {
                let priority = self.default_priority();
                rule.attributes.push(RuleAttribute::Priority(priority));
                RuleMessageWithPriority { rule_message: rule, priority }
            }
        }
    }

    /// Adds the given rule to the table.
    ///
    /// If successful, returns the index within the given priority set at which the rule was added.
    fn add_rule(&mut self, rule: RuleMessageWithPriority) -> Result<u16, AddRuleError> {
        // Get the rule's priority, setting it to a default if unset.
        let RuleMessageWithPriority { rule_message: rule, priority } = rule;

        let rules_at_priority = self.rules.entry(priority).or_default();
        let index: u16 = rules_at_priority.add_rule(rule)?;

        Ok(index)
    }

    /// Deletes the first rule from the table that matches the given pattern.
    fn del_first_matching_rule(
        &mut self,
        del_pattern: &RuleMessage,
    ) -> Result<(RulePriority, u16, RuleMessage), DelRuleError> {
        if !is_valid_del_pattern(del_pattern) {
            return Err(DelRuleError::InvalidPattern);
        }

        let bounds = if let Some(priority) = get_priority(del_pattern) {
            (std::ops::Bound::Included(priority), std::ops::Bound::Included(priority))
        } else {
            (std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
        };

        fn remove_rule_in_bounds(
            rules: &mut BTreeMap<RulePriority, IndexedRules>,
            del_pattern: &RuleMessage,
            bounds: (std::ops::Bound<RulePriority>, std::ops::Bound<RulePriority>),
        ) -> Option<(RulePriority, u16, RuleMessage)> {
            for (priority, rules) in rules.range_mut(bounds) {
                if let Some((index, rule)) = rules.remove_first_matching(del_pattern) {
                    return Some((*priority, index, rule));
                }
            }
            None
        }

        if let Some((priority, index, rule)) =
            remove_rule_in_bounds(&mut self.rules, del_pattern, bounds)
        {
            if self.rules.get(&priority).map(IndexedRules::is_empty).unwrap_or(false) {
                // Garbage collect the empty `IndexedRules`.
                let _: Option<IndexedRules> = self.rules.remove(&priority);
            }
            Ok((priority, index, rule))
        } else {
            Err(DelRuleError::NoMatchesForPattern)
        }
    }

    /// Deletes the rule at the given priority and index.
    fn del_rule(&mut self, priority: RulePriority, index: u16) -> Option<RuleMessage> {
        let rules = self.rules.get_mut(&priority)?;
        if let Some(rule) = rules.remove(index) {
            if rules.is_empty() {
                assert_matches!(self.rules.remove(&priority), Some(_));
            }
            Some(rule)
        } else {
            None
        }
    }

    /// Iterate over all the rules.
    ///
    /// The rules are ordered first by [`RulePriority`], and second by age.
    fn iter_rules(&self) -> impl Iterator<Item = &RuleMessage> {
        self.rules.values().flat_map(|rules| rules.iter())
    }

    /// Returns the default_priority to use for a newly installed rule.
    ///
    /// For conformance with Linux, new rules should have their priority set to
    /// `n-1` where n is the priority of the second rule in the table.
    fn default_priority(&self) -> RulePriority {
        if let Some(second_rule) = self.iter_rules().skip(1).next() {
            get_priority(second_rule)
                .expect("rules installed in the RuleTable must have a priority")
                .saturating_sub(1)
        } else {
            0
        }
    }
}

/// Handles asynchronous requests for managing the rule table.
#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub(crate) struct RulesWorker<I: IpExt> {
    /// The rule table.
    rule_table: RuleTable<I>,
    rule_set: <I::RuleSetMarker as ProtocolMarker>::Proxy,
}

impl<I: IpExt> RulesWorker<I> {
    /// Constructs an empty RulesWorker.
    pub(crate) fn new(rule_set: <I::RuleSetMarker as ProtocolMarker>::Proxy) -> RulesWorker<I> {
        RulesWorker { rule_table: RuleTable::<I>::new(), rule_set }
    }

    /// Constructs a RulesWorker prepopulated with the default rules present on
    /// Linux.
    /// * [V4] 0:        from all lookup local
    /// * [V4] 32766:    from all lookup main
    /// * [V4] 32767:    from all lookup default
    /// * [V6] 0:        from all lookup local
    /// * [V6] 32766:    from all lookup main
    pub(crate) async fn new_with_defaults(
        rule_set: <I::RuleSetMarker as ProtocolMarker>::Proxy,
        route_table_map: &mut RouteTableMap<I>,
    ) -> Result<RulesWorker<I>, AddRuleError> {
        struct NoIif;
        impl conversions::LookupIfInterfaceIsLoopback for NoIif {
            fn is_loopback(&self, _interface: &str) -> Option<bool> {
                panic!("should never be invoked as no default rules use `iif`");
            }
        }

        let mut table = RulesWorker::<I>::new(rule_set);
        for rule in RuleTable::<I>::default_rules() {
            table
                // None of the rules specify `iif`.
                .add_rule(rule, route_table_map, &NoIif)
                .await?
        }

        Ok(table)
    }

    pub(crate) async fn create(
        provider: &<I::RuleTableMarker as ProtocolMarker>::Proxy,
        route_table_map: &mut RouteTableMap<I>,
    ) -> Self {
        const MAX_ATTEMPTS: u32 = 10;
        let mut delay = Duration::from_millis(100);
        // Retry for up to 10 times, in case the asynchronous rule set removal is not yet complete.
        // The maximum time before giving up on creating a rule set is 100ms * (2 ^ 10 - 1) = 102.3s
        for _ in 0..MAX_ATTEMPTS {
            let rule_set =
                fnet_routes_ext::rules::new_rule_set::<I>(provider, NETLINK_RULE_SET_PRIORITY)
                    .expect("new rule set should succeed");
            match Self::new_with_defaults(rule_set, route_table_map).await {
                Ok(table) => return table,
                Err(AddRuleError::PriorityConflict) => {
                    log_warn!(
                        "priority conflict when creating rule set at {NETLINK_RULE_SET_PRIORITY:?}"
                    )
                }
                Err(e) => panic!("failed to add initial rules {e:?}"),
            }
            fuchsia_async::Timer::new(delay).await;
            delay *= 2;
        }
        panic!("failed to create rule set after {MAX_ATTEMPTS} retries");
    }

    /// Adds the given rule to the table.
    /// `interface_properties` are needed in case the rule specifies an `iif` (input interface), as
    /// we'd need to check whether it is loopback.
    async fn add_rule(
        &mut self,
        rule: RuleMessage,
        route_table_map: &mut RouteTableMap<I>,
        interface_properties: &impl conversions::LookupIfInterfaceIsLoopback,
    ) -> Result<(), AddRuleError> {
        let rule = self.rule_table.apply_default_priority_if_needed(rule);
        let priority = rule.priority;
        let index = self.rule_table.add_rule(rule.clone())?;

        match self
            .install_fidl_rule(rule.rule_message, index, route_table_map, interface_properties)
            .await
        {
            Ok(()) => (),
            Err(e) => {
                let _rule = self.rule_table.del_rule(priority, index);
                return Err(e);
            }
        }

        Ok(())
    }

    async fn install_fidl_rule(
        &self,
        rule: RuleMessage,
        index: u16,
        route_table_map: &mut RouteTableMap<I>,
        interface_properties: &impl conversions::LookupIfInterfaceIsLoopback,
    ) -> Result<(), AddRuleError> {
        let conversions::FidlRule { index, matcher, action } =
            conversions::fidl_rule_from_rule_message::<I>(&rule, index, interface_properties)
                .map_err(|e| {
                    crate::logging::log_warn!("failed to convert rule {rule:?} to FIDL: {e:?}");

                    use conversions::FidlRuleConversionError as E;
                    match e {
                        E::IpVersionMismatch
                        | E::UnsupportedAction
                        | E::InvalidTable
                        | E::InvalidSourceSubnet => AddRuleError::InvalidRule,
                        E::RuleIndexOverflow => AddRuleError::IndicesExhausted,
                        E::NoSuchInterface => AddRuleError::NoSuchInterface,
                    }
                })?;

        let action = match action {
            conversions::Action::Unreachable => fnet_routes_ext::rules::RuleAction::Unreachable,
            conversions::Action::ToTable(table_index) => {
                route_table_map.create_managed_route_table_if_not_present(table_index).await;
                let (table, fidl_table_id, rule_set_authenticated) =
                    match route_table_map.get_mut(&table_index).expect("should have just inserted")
                    {
                        RouteTable::Unmanaged(UnmanagedTable {
                            route_table_proxy,
                            fidl_table_id,
                            rule_set_authenticated,
                            ..
                        }) => (&*route_table_proxy, *fidl_table_id, rule_set_authenticated),
                        RouteTable::Managed(ManagedRouteTable {
                            route_table_proxy,
                            fidl_table_id,
                            rule_set_authenticated,
                            ..
                        }) => (&*route_table_proxy, *fidl_table_id, rule_set_authenticated),
                    };
                if !*rule_set_authenticated {
                    let GrantForRouteTableAuthorization { table_id, token } =
                        fnet_routes_ext::admin::get_authorization_for_route_table::<I>(table)
                            .await
                            .expect("should not get FIDL error");
                    fnet_routes_ext::rules::authenticate_for_route_table::<I>(
                        &self.rule_set,
                        table_id,
                        token,
                    )
                    .await
                    .map_err(|e| match e {
                        // We allow bubbling up error that indicates a priority conflict
                        // to allow the caller to retry.
                        fidl::Error::ClientChannelClosed { epitaph, .. }
                            if epitaph == fidl::Status::ALREADY_EXISTS =>
                        {
                            AddRuleError::PriorityConflict
                        }
                        _ => {
                            panic!("should not get FIDL error: {e:?}");
                        }
                    })?
                    .expect("authentication should be valid");
                    *rule_set_authenticated = true;
                }
                fnet_routes_ext::rules::RuleAction::Lookup(fidl_table_id)
            }
        };

        fnet_routes_ext::rules::add_rule(&self.rule_set, index, matcher, action.into())
            .await
            .expect("should not get FIDL error")
            .map_err(AddRuleError::RuleSetError)
    }

    /// Deletes the first rule from the table that matches the given pattern.
    async fn del_rule(&mut self, del_pattern: &RuleMessage) -> Result<RuleMessage, DelRuleError> {
        if !is_valid_del_pattern(del_pattern) {
            return Err(DelRuleError::InvalidPattern);
        }

        let (priority, index, rule) = self.rule_table.del_first_matching_rule(del_pattern)?;
        match self.uninstall_fidl_rule(priority, index).await {
            Ok(()) => (),
            Err(e) => {
                use RuleSetError as E;
                match e {
                    E::RuleDoesNotExist => {
                        crate::logging::log_error!(
                            "tried to uninstall nonexistent rule {rule:?} \
                             (priority={priority} index={index})"
                        );
                    }
                    E::Unauthenticated => panic!(
                        "should already be authenticated to delete rule we previously installed"
                    ),
                    E::RuleAlreadyExists | E::InvalidAction | E::InvalidMatcher => {
                        panic!("should not get RuleSetError {e:?} while deleting")
                    }
                    E::__SourceBreaking { unknown_ordinal } => {
                        crate::logging::log_error!(
                            "unknown RuleSetError (ordinal={unknown_ordinal}) \
                            while deleting rule {rule:?} \
                            (priority={priority:?}) (index={index:?})"
                        );
                    }
                }
            }
        }
        Ok(rule)
    }

    async fn uninstall_fidl_rule(&self, priority: u32, index: u16) -> Result<(), RuleSetError> {
        fnet_routes_ext::rules::remove_rule::<I>(
            &self.rule_set,
            conversions::compute_rule_index(priority, index)
                .expect("index of installed rule could not have overflowed"),
        )
        .await
        .expect("should not get FIDL error")
    }

    /// Iterate over all the rules.
    ///
    /// The rules are ordered first by [`RulePriority`], and second by age.
    fn iter_rules(&self) -> impl Iterator<Item = &RuleMessage> {
        self.rule_table.iter_rules()
    }

    pub(crate) fn any_rules_reference_table(&self, table_index: NetlinkRouteTableIndex) -> bool {
        self.iter_rules().any(|rule| {
            conversions::get_referenced_table(rule)
                .map(|table| table == table_index)
                .unwrap_or(false)
        })
    }
}

/// Possible errors when adding a rule to a [`RuleTable`].
#[derive(Debug)]
pub(crate) enum AddRuleError {
    AlreadyExists,
    PriorityConflict,
    IndicesExhausted,
    InvalidRule,
    NoSuchInterface,
    RuleSetError(RuleSetError),
}

impl AddRuleError {
    fn errno(&self) -> Errno {
        match self {
            AddRuleError::AlreadyExists => Errno::EEXIST,
            AddRuleError::IndicesExhausted => Errno::ETOOMANYREFS,
            AddRuleError::InvalidRule => Errno::EINVAL,
            AddRuleError::NoSuchInterface => Errno::ENODEV,
            AddRuleError::PriorityConflict => panic!("should handle rule set conflict on creation"),
            AddRuleError::RuleSetError(e) => match e {
                RuleSetError::Unauthenticated => panic!("should handle authentication prior"),
                RuleSetError::InvalidAction => Errno::EINVAL,
                RuleSetError::RuleAlreadyExists => Errno::EEXIST,
                RuleSetError::RuleDoesNotExist => {
                    panic!("should not get RuleDoesNotExist while adding")
                }
                RuleSetError::InvalidMatcher => Errno::EINVAL,
                RuleSetError::__SourceBreaking { unknown_ordinal: _ } => Errno::EINVAL,
            },
        }
    }
}

/// Possible errors when deleting a rule from a [`RuleTable`].
#[derive(Debug)]
enum DelRuleError {
    NoMatchesForPattern,
    InvalidPattern,
}

impl DelRuleError {
    fn errno(&self) -> Errno {
        match self {
            DelRuleError::NoMatchesForPattern => Errno::ENOENT,
            DelRuleError::InvalidPattern => Errno::ENOTSUP,
        }
    }
}

/// The set of possible requests related to PBR rules.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RuleRequestArgs {
    /// A RTM_GETRULE request with the NLM_F_DUMP flag set.
    /// Note that non-dump RTM_GETRULE requests are not supported by Netlink
    /// (this is also true on Linux).
    DumpRules,
    // A RTM_NEWRULE request. Holds the rule to be added.
    New(RuleMessage),
    // A RTM_DELRULE request. Holds the rule to be deleted.
    Del(RuleMessage),
}

/// A Netlink request related to PBR rules.
#[derive(Derivative, GenericOverIp)]
#[derivative(Debug(bound = ""))]
#[generic_over_ip(I, Ip)]
pub(crate) struct RuleRequest<S: Sender<<NetlinkRoute as ProtocolFamily>::Response>, I: Ip> {
    /// The arguments for this request.
    pub(crate) args: RuleRequestArgs,
    /// The request's sequence number.
    pub(crate) sequence_number: u32,
    /// The client that made the request.
    pub(crate) client: InternalClient<NetlinkRoute, S>,
    /// The IP Version of this request.
    pub(crate) _ip_version_marker: IpVersionMarker<I>,
}

impl<I: IpExt> RulesWorker<I> {
    pub(crate) async fn handle_request<S: Sender<<NetlinkRoute as ProtocolFamily>::Response>>(
        &mut self,
        req: RuleRequest<S, I>,
        route_table_map: &mut RouteTableMap<I>,
        interface_properties: &BTreeMap<
            u64,
            fnet_interfaces_ext::PropertiesAndState<
                crate::interfaces::InterfaceState,
                fnet_interfaces_ext::AllInterest,
            >,
        >,
    ) -> Result<Option<TableNeedsCleanup>, Errno> {
        crate::logging::log_debug!("Handling netlink rules request: {req:?}");

        let RuleRequest { args, _ip_version_marker: _, sequence_number, mut client } = req;

        match args {
            RuleRequestArgs::DumpRules => {
                for rule in self.iter_rules() {
                    client.send_unicast(to_nlm_new_rule(rule.clone(), sequence_number, true));
                }
                Ok(None)
            }
            RuleRequestArgs::New(rule) => {
                self.add_rule(rule, route_table_map, interface_properties)
                    .await
                    .map_err(|e| e.errno())
                    .map(|()| None)
                // TODO(https://issues.fuchsia.dev/292587350): Notify
                // multicast groups of `RTM_NEWRULE`.
            }
            RuleRequestArgs::Del(del_pattern) => {
                let rule_message = self.del_rule(&del_pattern).await.map_err(|e| e.errno())?;
                if let Some(netlink_id) = conversions::get_referenced_table(&rule_message) {
                    if let Some(table) = route_table_map.get(&netlink_id) {
                        return Ok(Some(TableNeedsCleanup(table.fidl_table_id(), netlink_id)));
                    }
                }
                // TODO(https://issues.fuchsia.dev/292587350): Notify
                // multicast groups of `RTM_DELRULE`.
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use fuchsia_async as fasync;
    use ip_test_macro::ip_test;
    use linux_uapi::{
        AF_INET, AF_INET6, FR_ACT_TO_TBL, FR_ACT_UNREACHABLE, FR_ACT_UNSPEC,
        rt_class_t_RT_TABLE_COMPAT,
    };
    use net_types::ip::IpInvariant;
    use test_case::test_case;

    use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;

    use crate::messaging::testutil::{FakeSender, FakeSenderSink, SentMessage};
    use crate::route_tables::{NetlinkRouteTableIndex, RouteTable, TableNeedsCleanup};

    const DUMP_SEQUENCE_NUM: u32 = 999;
    const EMPTY_INTERFACE_PROPERTIES: BTreeMap<
        u64,
        fnet_interfaces_ext::PropertiesAndState<
            crate::interfaces::InterfaceState,
            fnet_interfaces_ext::AllInterest,
        >,
    > = BTreeMap::new();

    fn build_rule<I: Ip>(action: u32, nlas: Vec<RuleAttribute>) -> RuleMessage {
        // This conversion is safe because action is actually a u8,
        // but out binding generator incorrectly emits a u32.
        let action = action as u8;
        let mut rule = RuleMessage::default();
        rule.header.action = RuleAction::from(action);
        rule.header.family =
            if I::VERSION == IpVersion::V4 { AddressFamily::Inet } else { AddressFamily::Inet6 };
        rule.attributes = nlas;
        rule
    }

    fn build_rule_with_table<I: Ip>(
        action: u32,
        nlas: Vec<RuleAttribute>,
        table: NetlinkRouteTableIndex,
    ) -> RuleMessage {
        let mut rule = build_rule::<I>(action, nlas);
        let header_table =
            u8::try_from(table.get()).ok().unwrap_or(rt_class_t_RT_TABLE_COMPAT as u8);
        rule.header.table = header_table;
        rule.attributes.push(RuleAttribute::Table(table.get()));
        rule
    }

    /// Helper function to dump the rules in the rule table.
    fn dump_rules<I: IpExt>(
        sink: &mut FakeSenderSink<RouteNetlinkMessage>,
        mut client: InternalClient<NetlinkRoute, FakeSender<RouteNetlinkMessage>>,
        table: &mut RuleTable<I>,
    ) -> Vec<NetlinkMessage<RouteNetlinkMessage>> {
        for rule in table.iter_rules() {
            client.send_unicast(to_nlm_new_rule(
                rule.clone(),
                DUMP_SEQUENCE_NUM,
                true, /* dump */
            ));
        }
        sink.take_messages().into_iter().map(|SentMessage { message, group: _ }| message).collect()
    }

    #[test_case(None)]
    #[test_case(Some(1))]
    fn test_get_priority(priority: Option<RulePriority>) {
        let mut rule = RuleMessage::default();
        if let Some(priority) = priority {
            rule.attributes.push(RuleAttribute::Priority(priority));
        }
        assert_eq!(get_priority(&rule), priority);
    }
    // Casts to u8 are safe as these are constants which fit into a u8.
    #[test_case(
        build_rule::<Ipv4>(FR_ACT_UNSPEC, vec![]),
        build_rule::<Ipv4>(FR_ACT_TO_TBL, vec![]),
        false; "different headers v4"
    )]
    #[test_case(
        build_rule::<Ipv4>(FR_ACT_TO_TBL, vec![RuleAttribute::Priority(1)]),
        build_rule::<Ipv4>(FR_ACT_TO_TBL, vec![]),
        false; "different nlas v4"
    )]
    #[test_case(
        build_rule::<Ipv4>(FR_ACT_TO_TBL, vec![RuleAttribute::Priority(1)]),
        build_rule::<Ipv4>(FR_ACT_TO_TBL, vec![RuleAttribute::Priority(1)]),
        true; "same header and nlas v4"
    )]
    #[test_case(
        build_rule::<Ipv4>(FR_ACT_TO_TBL, vec![RuleAttribute::Oifname(String::from("lo")), RuleAttribute::Priority(1)]),
        build_rule::<Ipv4>(FR_ACT_TO_TBL, vec![RuleAttribute::Priority(1), RuleAttribute::Oifname(String::from("lo"))]),
        true; "different nla order v4"
    )]
    #[test_case(
        build_rule::<Ipv6>(FR_ACT_UNSPEC, vec![]),
        build_rule::<Ipv6>(FR_ACT_TO_TBL, vec![]),
        false; "different headers v6"
    )]
    #[test_case(
        build_rule::<Ipv6>(FR_ACT_TO_TBL, vec![RuleAttribute::Priority(1)]),
        build_rule::<Ipv6>(FR_ACT_TO_TBL, vec![]),
        false; "different nlas v6"
    )]
    #[test_case(
        build_rule::<Ipv6>(FR_ACT_TO_TBL, vec![RuleAttribute::Priority(1)]),
        build_rule::<Ipv6>(FR_ACT_TO_TBL, vec![RuleAttribute::Priority(1)]),
        true; "same header and nlas v6"
    )]
    #[test_case(
        build_rule::<Ipv6>(FR_ACT_TO_TBL, vec![RuleAttribute::Oifname(String::from("lo")), RuleAttribute::Priority(1)]),
        build_rule::<Ipv6>(FR_ACT_TO_TBL, vec![RuleAttribute::Priority(1), RuleAttribute::Oifname(String::from("lo"))]),
        true; "different nla order v6"
    )]
    fn test_rules_are_equal(rule1: RuleMessage, rule2: RuleMessage, equal: bool) {
        assert_eq!(rules_are_equal(&rule1, &rule2), equal);
    }

    #[test_case(FR_ACT_UNSPEC as u8, vec![], false; "no_action_and_no_nlas_is_invalid")]
    #[test_case(FR_ACT_TO_TBL as u8, vec![], true; "action_and_no_nlas_is_valid")]
    #[test_case(FR_ACT_UNSPEC as u8, vec![RuleAttribute::Priority(1)], true; "no_action_and_nlas_is_valid")]
    #[test_case(FR_ACT_UNSPEC as u8, vec![
        RuleAttribute::Priority(1),
        RuleAttribute::Oifname(String::from("lo")),
        ], true; "no_action_and_multiple nlas_is_valid")]
    #[test_case(FR_ACT_TO_TBL as u8, vec![RuleAttribute::Priority(1)], true; "action_and_nlas_is_valid")]
    fn test_is_valid_del_pattern(action: u8, nlas: Vec<RuleAttribute>, expect_valid: bool) {
        let rule = build_rule::<Ipv4>(action.into(), nlas);
        assert_eq!(is_valid_del_pattern(&rule), expect_valid);
    }

    #[test_case(
        FR_ACT_UNSPEC as u8, FR_ACT_TO_TBL as u8,
        vec![], vec![],
        false; "mismatched_action")]
    #[test_case(
        FR_ACT_TO_TBL as u8, FR_ACT_TO_TBL as u8,
        vec![], vec![RuleAttribute::Priority(1)],
        false; "absent_nla")]
    #[test_case(
        FR_ACT_TO_TBL as u8, FR_ACT_TO_TBL as u8,
        vec![RuleAttribute::Priority(2)], vec![RuleAttribute::Priority(1)],
        false; "mismatched_nla")]
    #[test_case(
        FR_ACT_TO_TBL as u8, FR_ACT_TO_TBL as u8,
        vec![RuleAttribute::Priority(1)], vec![RuleAttribute::Priority(1)],
        true; "exact_match")]
    #[test_case(
        FR_ACT_TO_TBL as u8, FR_ACT_UNSPEC as u8,
        vec![RuleAttribute::Priority(1)], vec![RuleAttribute::Priority(1)],
        true; "more_specific_action_matches")]
    #[test_case(
        FR_ACT_UNSPEC as u8, FR_ACT_UNSPEC as u8,
        vec![RuleAttribute::Priority(1), RuleAttribute::Oifname(String::from("lo"))], vec![RuleAttribute::Priority(1)],
        true; "more_specific_nla_matches")]
    fn test_rule_matches_del_pattern(
        rule_action: u8,
        pattern_action: u8,
        rule_nlas: Vec<RuleAttribute>,
        pattern_nlas: Vec<RuleAttribute>,
        expect_match: bool,
    ) {
        let rule = build_rule::<Ipv4>(rule_action.into(), rule_nlas);
        let pattern = build_rule::<Ipv4>(pattern_action.into(), pattern_nlas);
        assert_eq!(rule_matches_del_pattern(&rule, &pattern), expect_match);
    }

    #[ip_test(I)]
    #[test_case(&[], 0; "no_existing_rules_defaults_to_zero")]
    #[test_case(&[99], 0; "one_existing_rules_defaults_to_zero")]
    #[test_case(&[0, 100], 99; "two_existing_rules_defaults_to_second_minus_1")]
    #[test_case(&[0, 100, 200], 99; "three_existing_rules_defaults_to_second_minus_1")]
    #[test_case(&[0, 1], 0; "default_priority_duplicates_existing_priority")]
    #[test_case(&[0, 0], 0; "default_priority_saturates_at_0")]
    fn test_rule_table_default_priority<I: IpExt>(
        existing_rule_priorities: &[RulePriority],
        expected_default_priority: RulePriority,
    ) {
        let mut table = RuleTable::<I>::new();
        for (index, priority) in existing_rule_priorities.iter().enumerate() {
            // Give each rule a different `OifName` to avoid "already exists"
            // conflicts.
            let name = RuleAttribute::Oifname(index.to_string());
            let _index = table
                .add_rule(
                    build_rule::<I>(FR_ACT_UNSPEC, vec![RuleAttribute::Priority(*priority), name])
                        .try_into()
                        .unwrap(),
                )
                .expect("add rule should succeed");
        }

        assert_eq!(table.default_priority(), expected_default_priority);
    }

    #[ip_test(I)]
    fn test_rule_table_frees_unused_priorities<I: IpExt>() {
        let mut table = RuleTable::<I>::new();
        const PRIORITY: RulePriority = 99;
        let rule = build_rule::<I>(FR_ACT_UNSPEC, vec![RuleAttribute::Priority(PRIORITY)]);

        let _index =
            table.add_rule(rule.clone().try_into().unwrap()).expect("add rule should succeed");
        assert!(table.rules.contains_key(&PRIORITY));
        // Remove the rule, and verify the priority was removed (as opposed to
        // still existing and holding an empty vec).
        let (_priority, _index, _message) =
            table.del_first_matching_rule(&rule).expect("del rule should succeed");
        assert!(!table.rules.contains_key(&PRIORITY));
    }

    const MAIN_FIDL_TABLE_ID: fnet_routes_ext::TableId = fnet_routes_ext::TableId::new(0);
    const MANAGED_FIDL_TABLE_ID: fnet_routes_ext::TableId = fnet_routes_ext::TableId::new(42);
    const MANAGED_NETLINK_TABLE_ID: NetlinkRouteTableIndex = NetlinkRouteTableIndex::new(888);

    fn test_route_table_map<I: IpExt>() -> RouteTableMap<I> {
        let (main_route_table_proxy, _server_end) =
            fidl::endpoints::create_proxy::<I::RouteTableMarker>();
        let (unmanaged_route_set_proxy, _unmanaged_route_set_server_end) =
            fidl::endpoints::create_proxy::<I::RouteSetMarker>();
        let (route_table_provider, _server_end) =
            fidl::endpoints::create_proxy::<I::RouteTableProviderMarker>();

        RouteTableMap::<I>::new(
            main_route_table_proxy,
            MAIN_FIDL_TABLE_ID,
            unmanaged_route_set_proxy,
            route_table_provider,
        )
    }

    #[test_case(<Ipv4 as Ip>::VERSION_MARKER; "v4")]
    #[test_case(<Ipv6 as Ip>::VERSION_MARKER; "v6")]
    #[fuchsia::test]
    async fn test_rule_table<I: IpExt>(ip_version_marker: IpVersionMarker<I>) {
        let scope = fasync::Scope::new();
        let (mut sink, client, async_work_drain_task) = crate::client::testutil::new_fake_client(
            crate::client::testutil::CLIENT_ID_1,
            std::iter::empty(),
        );
        let _join_handle = scope.spawn(async_work_drain_task);
        let (rule_set, server_end) = fidl::endpoints::create_proxy::<I::RuleSetMarker>();
        let _serve_task = fuchsia_async::Task::local(
            fnet_routes_ext::testutil::rules::serve_noop_rule_set::<I>(server_end),
        );
        let mut table = RulesWorker::<I>::new(rule_set);

        // Verify that the table is empty.
        assert_eq!(&dump_rules(&mut sink, client.clone(), &mut table.rule_table)[..], &[],);

        const LOW_PRIORITY: RulePriority = 100;
        const HIGH_PRIORITY: RulePriority = 200;

        let (own_route_table_proxy, _server_end) =
            fidl::endpoints::create_proxy::<I::RouteTableMarker>();
        let (route_set_proxy, _server_end) = fidl::endpoints::create_proxy::<I::RouteSetMarker>();

        let mut route_table_map = test_route_table_map::<I>();

        // We should pre-populate the table so that we don't need to create a new one.
        route_table_map.insert(
            MANAGED_NETLINK_TABLE_ID,
            RouteTable::Managed(ManagedRouteTable {
                route_table_proxy: own_route_table_proxy,
                route_set_proxy,
                fidl_table_id: MANAGED_FIDL_TABLE_ID,
                rule_set_authenticated: true,
            }),
        );

        // Add a new rule and expect success.
        // Conversion is safe as FR_ACT_TO_TBL (1), fits into a u8.
        let low_priority_rule = build_rule_with_table::<I>(
            FR_ACT_TO_TBL,
            vec![RuleAttribute::Priority(LOW_PRIORITY)],
            MANAGED_NETLINK_TABLE_ID,
        );
        let cleanup = table
            .handle_request(
                RuleRequest {
                    args: RuleRequestArgs::New(low_priority_rule.clone()),
                    _ip_version_marker: ip_version_marker,
                    sequence_number: 0,
                    client: client.clone(),
                },
                &mut route_table_map,
                &EMPTY_INTERFACE_PROPERTIES,
            )
            .await
            .expect("new rule should succeed");
        assert_eq!(
            &dump_rules(&mut sink, client.clone(), &mut table.rule_table)[..],
            &[to_nlm_new_rule(low_priority_rule.clone(), DUMP_SEQUENCE_NUM, true)]
        );
        assert_eq!(cleanup, None);

        // Adding a "different" rule with the same priority should succeed.
        let newer_low_priority_rule = build_rule_with_table::<I>(
            FR_ACT_TO_TBL,
            vec![RuleAttribute::Priority(LOW_PRIORITY), RuleAttribute::Oifname(String::from("lo"))],
            MANAGED_NETLINK_TABLE_ID,
        );
        let cleanup = table
            .handle_request(
                RuleRequest {
                    args: RuleRequestArgs::New(newer_low_priority_rule.clone()),
                    _ip_version_marker: ip_version_marker,
                    sequence_number: 0,
                    client: client.clone(),
                },
                &mut route_table_map,
                &EMPTY_INTERFACE_PROPERTIES,
            )
            .await
            .expect("new rule should succeed");
        assert_eq!(
            &dump_rules(&mut sink, client.clone(), &mut table.rule_table)[..],
            // Ordered oldest to newest
            &[
                to_nlm_new_rule(low_priority_rule.clone(), DUMP_SEQUENCE_NUM, true),
                to_nlm_new_rule(newer_low_priority_rule.clone(), DUMP_SEQUENCE_NUM, true),
            ]
        );
        assert_eq!(cleanup, None);

        // Adding the "same" rule with a different priority should succeed.
        let high_priority_rule = build_rule_with_table::<I>(
            FR_ACT_TO_TBL,
            vec![RuleAttribute::Priority(HIGH_PRIORITY)],
            MANAGED_NETLINK_TABLE_ID,
        );
        let cleanup = table
            .handle_request(
                RuleRequest {
                    args: RuleRequestArgs::New(high_priority_rule.clone()),
                    _ip_version_marker: ip_version_marker,
                    sequence_number: 0,
                    client: client.clone(),
                },
                &mut route_table_map,
                &EMPTY_INTERFACE_PROPERTIES,
            )
            .await
            .expect("new rule should succeed");
        assert_eq!(
            &dump_rules(&mut sink, client.clone(), &mut table.rule_table)[..],
            &[
                // Ordered in ascending priority
                to_nlm_new_rule(low_priority_rule.clone(), DUMP_SEQUENCE_NUM, true),
                to_nlm_new_rule(newer_low_priority_rule.clone(), DUMP_SEQUENCE_NUM, true),
                to_nlm_new_rule(high_priority_rule.clone(), DUMP_SEQUENCE_NUM, true),
            ]
        );
        assert_eq!(cleanup, None);

        // Specify a deletion pattern that matches all three existing rules, and
        // expect the oldest, lowest priority rule to be removed first.
        let del_pattern_match_all =
            build_rule_with_table::<I>(FR_ACT_TO_TBL, vec![], MANAGED_NETLINK_TABLE_ID);
        let cleanup = table
            .handle_request(
                RuleRequest {
                    args: RuleRequestArgs::Del(del_pattern_match_all.clone()),
                    _ip_version_marker: ip_version_marker,
                    sequence_number: 0,
                    client: client.clone(),
                },
                &mut route_table_map,
                &EMPTY_INTERFACE_PROPERTIES,
            )
            .await
            .expect("del rule should succeed");
        assert_eq!(
            &dump_rules(&mut sink, client.clone(), &mut table.rule_table)[..],
            &[
                to_nlm_new_rule(newer_low_priority_rule.clone(), DUMP_SEQUENCE_NUM, true),
                to_nlm_new_rule(high_priority_rule.clone(), DUMP_SEQUENCE_NUM, true),
            ]
        );
        assert_eq!(
            cleanup,
            Some(TableNeedsCleanup(MANAGED_FIDL_TABLE_ID, MANAGED_NETLINK_TABLE_ID))
        );

        // Specify a deletion pattern that only matches the high_priority_rule,
        // and expect it to be deleted.
        // Conversion is safe as FR_ACT_TO_TBL (1) fits into a u8.
        let del_pattern_match_high_priority = build_rule_with_table::<I>(
            FR_ACT_TO_TBL,
            vec![RuleAttribute::Priority(HIGH_PRIORITY)],
            MANAGED_NETLINK_TABLE_ID,
        );
        let cleanup = table
            .handle_request(
                RuleRequest {
                    args: RuleRequestArgs::Del(del_pattern_match_high_priority),
                    _ip_version_marker: ip_version_marker,
                    sequence_number: 0,
                    client: client.clone(),
                },
                &mut route_table_map,
                &EMPTY_INTERFACE_PROPERTIES,
            )
            .await
            .expect("del rule should succeed");
        assert_eq!(
            &dump_rules(&mut sink, client.clone(), &mut table.rule_table)[..],
            &[to_nlm_new_rule(newer_low_priority_rule.clone(), DUMP_SEQUENCE_NUM, true)]
        );
        assert_eq!(
            cleanup,
            Some(TableNeedsCleanup(MANAGED_FIDL_TABLE_ID, MANAGED_NETLINK_TABLE_ID))
        );

        // Delete the final rule.
        // Conversion is safe as FR_ACT_TO_TBL (1) fits into a u8.
        let del_pattern_match_all =
            build_rule_with_table::<I>(FR_ACT_TO_TBL, vec![], MANAGED_NETLINK_TABLE_ID);
        let cleanup = table
            .handle_request(
                RuleRequest {
                    args: RuleRequestArgs::Del(del_pattern_match_all),
                    _ip_version_marker: ip_version_marker,
                    sequence_number: 0,
                    client: client.clone(),
                },
                &mut route_table_map,
                &EMPTY_INTERFACE_PROPERTIES,
            )
            .await
            .expect("del rule should succeed");
        assert_eq!(&dump_rules(&mut sink, client.clone(), &mut table.rule_table)[..], &[]);
        assert_eq!(
            cleanup,
            Some(TableNeedsCleanup(MANAGED_FIDL_TABLE_ID, MANAGED_NETLINK_TABLE_ID))
        );
        drop(client);
        scope.join().await;
    }

    #[test_case(<Ipv4 as Ip>::VERSION_MARKER; "v4")]
    #[test_case(<Ipv6 as Ip>::VERSION_MARKER; "v6")]
    #[fuchsia::test]
    async fn test_rule_table_new_rule_already_exists<I: IpExt>(
        ip_version_marker: IpVersionMarker<I>,
    ) {
        let (mut sink, client, async_work_drain_task) = crate::client::testutil::new_fake_client(
            crate::client::testutil::CLIENT_ID_1,
            std::iter::empty(),
        );
        let scope = fasync::Scope::new();
        let _join_handle = scope.spawn(async_work_drain_task);
        let (rule_set, server_end) = fidl::endpoints::create_proxy::<I::RuleSetMarker>();
        let _serve_task = fuchsia_async::Task::local(
            fnet_routes_ext::testutil::rules::serve_noop_rule_set::<I>(server_end),
        );
        let mut table = RulesWorker::new(rule_set);

        const PRIORITY_NLA: RuleAttribute = RuleAttribute::Priority(0);
        let oif_nla = RuleAttribute::Oifname(String::from("lo"));

        let mut route_table_map = test_route_table_map::<I>();

        // Add a new rule and expect success.
        // Conversion is safe as FR_ACT_UNREACHABLE (7) fits into a u8.
        let rule = build_rule::<I>(FR_ACT_UNREACHABLE, vec![oif_nla.clone(), PRIORITY_NLA]);
        let cleanup = table
            .handle_request(
                RuleRequest {
                    args: RuleRequestArgs::New(rule.clone()),
                    _ip_version_marker: ip_version_marker,
                    sequence_number: 0,
                    client: client.clone(),
                },
                &mut route_table_map,
                &EMPTY_INTERFACE_PROPERTIES,
            )
            .await
            .expect("new rule should succeed");
        assert_eq!(
            &dump_rules(&mut sink, client.clone(), &mut table.rule_table)[..],
            &[to_nlm_new_rule(rule.clone(), DUMP_SEQUENCE_NUM, true)]
        );
        assert_eq!(cleanup, None);

        // Adding the same rule should return EEXIST.
        let result = table
            .handle_request(
                RuleRequest {
                    args: RuleRequestArgs::New(rule.clone()),
                    _ip_version_marker: ip_version_marker,
                    sequence_number: 0,
                    client: client.clone(),
                },
                &mut route_table_map,
                &EMPTY_INTERFACE_PROPERTIES,
            )
            .await;
        assert_eq!(result, Err(Errno::EEXIST));

        // Adding the same rule with out-of-order NLAs should return EEXIST.
        // Conversion is safe as FR_ACT_UNREACHABLE (7) fits into a u8.
        let out_of_order_rule =
            build_rule::<I>(FR_ACT_UNREACHABLE, vec![PRIORITY_NLA, oif_nla.clone()]);
        let result = table
            .handle_request(
                RuleRequest {
                    args: RuleRequestArgs::New(out_of_order_rule),
                    _ip_version_marker: ip_version_marker,
                    sequence_number: 0,
                    client: client.clone(),
                },
                &mut route_table_map,
                &EMPTY_INTERFACE_PROPERTIES,
            )
            .await;
        assert_eq!(result, Err(Errno::EEXIST));

        // Try again, but erase the `Priority` NLA. This confirms EEXIST is
        // still reported when the default priority would conflict with an
        // existing_rule
        // Conversion is safe as FR_ACT_UNREACHABLE (7) fits into a u8.
        let rule_without_priority = build_rule::<I>(FR_ACT_UNREACHABLE, vec![oif_nla.clone()]);
        let result = table
            .handle_request(
                RuleRequest {
                    args: RuleRequestArgs::New(rule_without_priority),
                    _ip_version_marker: ip_version_marker,
                    sequence_number: 0,
                    client: client.clone(),
                },
                &mut route_table_map,
                &EMPTY_INTERFACE_PROPERTIES,
            )
            .await;
        assert_eq!(result, Err(Errno::EEXIST));
        drop(client);
        scope.join().await;
    }

    // Conversions are safe as these constants fit into a u8.
    #[test_case(RuleMessage::default(), Errno::ENOTSUP, <Ipv4 as Ip>::VERSION_MARKER;
        "empty_patern_not_supported_v4")]
    #[test_case(build_rule::<Ipv4>(FR_ACT_TO_TBL, vec![]), Errno::ENOENT, <Ipv4 as Ip>::VERSION_MARKER;
        "no_matching_rules_v4")]
    #[test_case(RuleMessage::default(), Errno::ENOTSUP, <Ipv6 as Ip>::VERSION_MARKER;
        "empty_patern_not_supported_v6")]
    #[test_case(build_rule::<Ipv6>(FR_ACT_TO_TBL, vec![]), Errno::ENOENT, <Ipv6 as Ip>::VERSION_MARKER;
        "no_matching_rules_v6")]
    #[fuchsia::test]
    async fn test_rule_table_del_rule_fails<I: IpExt>(
        pattern: RuleMessage,
        error: Errno,
        ip_version_marker: IpVersionMarker<I>,
    ) {
        let (mut sink, client, async_work_drain_task) = crate::client::testutil::new_fake_client(
            crate::client::testutil::CLIENT_ID_1,
            std::iter::empty(),
        );
        let scope = fasync::Scope::new();
        let _join_handle = scope.spawn(async_work_drain_task);
        let (rule_set, _server_end) = fidl::endpoints::create_proxy::<I::RuleSetMarker>();
        let mut table = RulesWorker::new(rule_set);
        assert_eq!(&dump_rules(&mut sink, client.clone(), &mut table.rule_table)[..], &[]);

        let mut route_table_map = test_route_table_map::<I>();

        let result = table
            .handle_request(
                RuleRequest {
                    args: RuleRequestArgs::Del(pattern),
                    _ip_version_marker: ip_version_marker,
                    sequence_number: 0,
                    client,
                },
                &mut route_table_map,
                &EMPTY_INTERFACE_PROPERTIES,
            )
            .await;
        assert_eq!(result, Err(error));
        scope.join().await;
    }

    #[ip_test(I, test = false)]
    #[fuchsia::test]
    async fn test_default_rules<I: IpExt>() {
        let scope = fasync::Scope::new();
        let (mut sink, client, async_work_drain_task) = crate::client::testutil::new_fake_client(
            crate::client::testutil::CLIENT_ID_1,
            std::iter::empty(),
        );
        let _join_handle = scope.spawn(async_work_drain_task);

        let mut table = RuleTable::<I>::new_with_defaults();

        let new_rule = |table: u8, priority: RulePriority, family: u16| {
            let mut rule = RuleMessage::default();
            rule.header.action = RuleAction::ToTable;
            rule.header.table = table;
            // Conversion is safe as family is guaranteed to fit into a u8.
            rule.header.family = AddressFamily::from(family as u8);
            rule.attributes = vec![RuleAttribute::Priority(priority)];
            to_nlm_new_rule(rule, DUMP_SEQUENCE_NUM, true)
        };
        // Conversion is safe as these compile-time constants are guaranteed to fit into a u8.

        I::map_ip_in(
            (IpInvariant((&mut sink, client.clone())), &mut table),
            |(IpInvariant((sink, client)), table)| {
                assert_eq!(
                    &dump_rules(sink, client, table)[..],
                    &[
                        new_rule(
                            rt_class_t_RT_TABLE_LOCAL as u8,
                            LINUX_DEFAULT_LOOKUP_LOCAL_PRIORITY,
                            AF_INET as u16
                        ),
                        new_rule(
                            rt_class_t_RT_TABLE_MAIN as u8,
                            LINUX_DEFAULT_LOOKUP_MAIN_PRIORITY,
                            AF_INET as u16
                        ),
                        new_rule(
                            rt_class_t_RT_TABLE_DEFAULT as u8,
                            LINUX_DEFAULT_LOOKUP_DEFAULT_PRIORITY,
                            AF_INET as u16
                        ),
                    ]
                );
            },
            |(IpInvariant((sink, client)), table)| {
                assert_eq!(
                    &dump_rules(sink, client, table)[..],
                    &[
                        new_rule(
                            rt_class_t_RT_TABLE_LOCAL as u8,
                            LINUX_DEFAULT_LOOKUP_LOCAL_PRIORITY,
                            AF_INET6 as u16
                        ),
                        new_rule(
                            rt_class_t_RT_TABLE_MAIN as u8,
                            LINUX_DEFAULT_LOOKUP_MAIN_PRIORITY,
                            AF_INET6 as u16
                        ),
                    ]
                );
            },
        );
        drop(client);
        scope.join().await;
    }
}
