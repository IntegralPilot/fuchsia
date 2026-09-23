// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extensions for route rules FIDL.

use std::fmt::Debug;

use async_utils::{fold, stream};
use fidl_fuchsia_net_ext::{IntoExt as _, TryIntoExt as _};
#[cfg(not(feature = "fdomain"))]
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
#[cfg(feature = "fdomain")]
use fidl_fuchsia_net_matchers_ext_fdomain as fnet_matchers_ext;
use flex_client::ProxyHasDomain as _;
use flex_client::fidl::{DiscoverableProtocolMarker, ProtocolMarker};
use flex_fuchsia_net as fnet;
use flex_fuchsia_net_matchers as fnet_matchers;
use flex_fuchsia_net_routes as fnet_routes;
use flex_fuchsia_net_routes_admin as fnet_routes_admin;
use futures::future::Either;
use futures::{Stream, TryStreamExt as _};
use net_types::ip::{GenericOverIp, Ip, Ipv4, Ipv6, Subnet};
use thiserror::Error;

use crate::{FidlRouteIpExt, Responder, SliceResponder, WatcherCreationError, impl_responder};

/// Observation extension for the rules part of `fuchsia.net.routes` FIDL API.
pub trait FidlRuleIpExt: Ip {
    /// The "rules watcher" protocol to use for this IP version.
    type RuleWatcherMarker: ProtocolMarker<RequestStream = Self::RuleWatcherRequestStream>;
    /// The "rules watcher" request stream.
    type RuleWatcherRequestStream: flex_client::fidl::RequestStream<Ok: Send, ControlHandle: Send>;
    /// The rule event to be watched.
    type RuleEvent: From<RuleEvent<Self>>
        + TryInto<RuleEvent<Self>, Error = RuleFidlConversionError>
        + Unpin;
    /// The responder to the watch request.
    type RuleWatcherWatchResponder: SliceResponder<Self::RuleEvent>;

    /// Turns a FIDL rule watcher request into the extension type.
    fn into_rule_watcher_request(
        request: flex_client::fidl::Request<Self::RuleWatcherMarker>,
    ) -> RuleWatcherRequest<Self>;
}

impl_responder!(fnet_routes::RuleWatcherV4WatchResponder, &[fnet_routes::RuleEventV4]);
impl_responder!(fnet_routes::RuleWatcherV6WatchResponder, &[fnet_routes::RuleEventV6]);

impl FidlRuleIpExt for Ipv4 {
    type RuleWatcherMarker = fnet_routes::RuleWatcherV4Marker;
    type RuleWatcherRequestStream = fnet_routes::RuleWatcherV4RequestStream;
    type RuleEvent = fnet_routes::RuleEventV4;
    type RuleWatcherWatchResponder = fnet_routes::RuleWatcherV4WatchResponder;

    fn into_rule_watcher_request(
        request: flex_client::fidl::Request<Self::RuleWatcherMarker>,
    ) -> RuleWatcherRequest<Self> {
        RuleWatcherRequest::from(request)
    }
}

impl FidlRuleIpExt for Ipv6 {
    type RuleWatcherMarker = fnet_routes::RuleWatcherV6Marker;
    type RuleWatcherRequestStream = fnet_routes::RuleWatcherV6RequestStream;
    type RuleEvent = fnet_routes::RuleEventV6;
    type RuleWatcherWatchResponder = fnet_routes::RuleWatcherV6WatchResponder;

    fn into_rule_watcher_request(
        request: flex_client::fidl::Request<Self::RuleWatcherMarker>,
    ) -> RuleWatcherRequest<Self> {
        RuleWatcherRequest::from(request)
    }
}

/// The request for the rules watchers.
pub enum RuleWatcherRequest<I: FidlRuleIpExt> {
    /// Hanging-Get style API for observing routing rule changes.
    Watch {
        /// Responder for the events.
        responder: I::RuleWatcherWatchResponder,
    },
}

impl From<fnet_routes::RuleWatcherV4Request> for RuleWatcherRequest<Ipv4> {
    fn from(req: fnet_routes::RuleWatcherV4Request) -> Self {
        match req {
            fnet_routes::RuleWatcherV4Request::Watch { responder } => {
                RuleWatcherRequest::Watch { responder }
            }
        }
    }
}

impl From<fnet_routes::RuleWatcherV6Request> for RuleWatcherRequest<Ipv6> {
    fn from(req: fnet_routes::RuleWatcherV6Request) -> Self {
        match req {
            fnet_routes::RuleWatcherV6Request::Watch { responder } => {
                RuleWatcherRequest::Watch { responder }
            }
        }
    }
}

/// An installed IPv4 routing rule.
#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct InstalledRule<I: Ip> {
    /// Rule sets are ordered by the rule set priority, rule sets are disjoint
    /// and don’t have interleaving rules among them.
    pub priority: RuleSetPriority,
    /// Rules within a rule set are locally ordered, together with the rule set
    /// priority, this defines a global order for all installed rules.
    pub index: RuleIndex,
    /// The matcher part of the rule, the rule is a no-op if the matcher does
    /// not match the packet.
    pub matcher: RuleMatcher<I>,
    /// The action part of the rule that describes what to do if the matcher
    /// matches the packet.
    pub action: RuleAction,
}

impl TryFrom<fnet_routes::InstalledRuleV4> for InstalledRule<Ipv4> {
    type Error = RuleFidlConversionError;
    fn try_from(
        fnet_routes::InstalledRuleV4 {
        rule_set_priority,
        rule_index,
        matcher,
        action,
    }: fnet_routes::InstalledRuleV4,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            priority: rule_set_priority.into(),
            index: rule_index.into(),
            matcher: matcher.try_into()?,
            action: action.into(),
        })
    }
}

impl TryFrom<fnet_routes::InstalledRuleV6> for InstalledRule<Ipv6> {
    type Error = RuleFidlConversionError;
    fn try_from(
        fnet_routes::InstalledRuleV6 {
        rule_set_priority,
        rule_index,
        matcher,
        action,
    }: fnet_routes::InstalledRuleV6,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            priority: rule_set_priority.into(),
            index: rule_index.into(),
            matcher: matcher.try_into()?,
            action: action.into(),
        })
    }
}

impl From<InstalledRule<Ipv4>> for fnet_routes::InstalledRuleV4 {
    fn from(InstalledRule { priority, index, matcher, action }: InstalledRule<Ipv4>) -> Self {
        Self {
            rule_set_priority: priority.into(),
            rule_index: index.into(),
            matcher: matcher.into(),
            action: action.into(),
        }
    }
}

impl From<InstalledRule<Ipv6>> for fnet_routes::InstalledRuleV6 {
    fn from(InstalledRule { priority, index, matcher, action }: InstalledRule<Ipv6>) -> Self {
        Self {
            rule_set_priority: priority.into(),
            rule_index: index.into(),
            matcher: matcher.into(),
            action: action.into(),
        }
    }
}

/// A rules watcher event.
#[derive(Debug, Clone)]
pub enum RuleEvent<I: Ip> {
    /// A rule that already existed when watching started.
    Existing(InstalledRule<I>),
    /// Sentinel value indicating no more `existing` events will be
    /// received.
    Idle,
    /// A rule that was added while watching.
    Added(InstalledRule<I>),
    /// A rule that was removed while watching.
    Removed(InstalledRule<I>),
}

impl TryFrom<fnet_routes::RuleEventV4> for RuleEvent<Ipv4> {
    type Error = RuleFidlConversionError;
    fn try_from(event: fnet_routes::RuleEventV4) -> Result<Self, Self::Error> {
        match event {
            fnet_routes::RuleEventV4::Existing(rule) => Ok(RuleEvent::Existing(rule.try_into()?)),
            fnet_routes::RuleEventV4::Idle(fnet_routes::Empty) => Ok(RuleEvent::Idle),
            fnet_routes::RuleEventV4::Added(rule) => Ok(RuleEvent::Added(rule.try_into()?)),
            fnet_routes::RuleEventV4::Removed(rule) => Ok(RuleEvent::Removed(rule.try_into()?)),
            fnet_routes::RuleEventV4::__SourceBreaking { unknown_ordinal } => {
                Err(RuleFidlConversionError::UnknownOrdinal {
                    name: "RuleEventV4",
                    unknown_ordinal,
                })
            }
        }
    }
}

impl TryFrom<fnet_routes::RuleEventV6> for RuleEvent<Ipv6> {
    type Error = RuleFidlConversionError;
    fn try_from(event: fnet_routes::RuleEventV6) -> Result<Self, Self::Error> {
        match event {
            fnet_routes::RuleEventV6::Existing(rule) => Ok(RuleEvent::Existing(rule.try_into()?)),
            fnet_routes::RuleEventV6::Idle(fnet_routes::Empty) => Ok(RuleEvent::Idle),
            fnet_routes::RuleEventV6::Added(rule) => Ok(RuleEvent::Added(rule.try_into()?)),
            fnet_routes::RuleEventV6::Removed(rule) => Ok(RuleEvent::Removed(rule.try_into()?)),
            fnet_routes::RuleEventV6::__SourceBreaking { unknown_ordinal } => {
                Err(RuleFidlConversionError::UnknownOrdinal {
                    name: "RuleEventV6",
                    unknown_ordinal,
                })
            }
        }
    }
}

impl From<RuleEvent<Ipv4>> for fnet_routes::RuleEventV4 {
    fn from(event: RuleEvent<Ipv4>) -> Self {
        match event {
            RuleEvent::Existing(r) => Self::Existing(r.into()),
            RuleEvent::Idle => Self::Idle(fnet_routes::Empty),
            RuleEvent::Added(r) => Self::Added(r.into()),
            RuleEvent::Removed(r) => Self::Removed(r.into()),
        }
    }
}

impl From<RuleEvent<Ipv6>> for fnet_routes::RuleEventV6 {
    fn from(event: RuleEvent<Ipv6>) -> Self {
        match event {
            RuleEvent::Existing(r) => Self::Existing(r.into()),
            RuleEvent::Idle => Self::Idle(fnet_routes::Empty),
            RuleEvent::Added(r) => Self::Added(r.into()),
            RuleEvent::Removed(r) => Self::Removed(r.into()),
        }
    }
}

/// Admin extension for the rules part of `fuchsia.net.routes.admin` FIDL API.
pub trait FidlRuleAdminIpExt: Ip {
    /// The "rule table" protocol to use for this IP version.
    type RuleTableMarker: DiscoverableProtocolMarker<RequestStream = Self::RuleTableRequestStream>;
    /// The "rule set" protocol to use for this IP Version.
    type RuleSetMarker: ProtocolMarker<RequestStream = Self::RuleSetRequestStream>;
    /// The request stream for the rule table protocol.
    type RuleTableRequestStream: flex_client::fidl::RequestStream<Ok: Send, ControlHandle: Send>;
    /// The request stream for the rule set protocol.
    type RuleSetRequestStream: flex_client::fidl::RequestStream<Ok: Send, ControlHandle: Send>;
    /// The responder for AddRule requests.
    type RuleSetAddRuleResponder: Responder<
            Payload = Result<(), fnet_routes_admin::RuleSetError>,
            ControlHandle = Self::RuleSetControlHandle,
        >;
    /// The responder for RemoveRule requests.
    type RuleSetRemoveRuleResponder: Responder<
            Payload = Result<(), fnet_routes_admin::RuleSetError>,
            ControlHandle = Self::RuleSetControlHandle,
        >;
    /// The responder for AuthenticateForRouteTable requests.
    type RuleSetAuthenticateForRouteTableResponder: Responder<
            Payload = Result<(), fnet_routes_admin::AuthenticateForRouteTableError>,
            ControlHandle = Self::RuleSetControlHandle,
        >;
    /// The control handle for RuleTable protocols.
    type RuleTableControlHandle: flex_client::fidl::ControlHandle + Send + Clone;
    /// The control handle for RuleSet protocols.
    type RuleSetControlHandle: flex_client::fidl::ControlHandle + Send + Clone;

    /// Turns a FIDL rule set request into the extension type.
    fn into_rule_set_request(
        request: flex_client::fidl::Request<Self::RuleSetMarker>,
    ) -> RuleSetRequest<Self>;

    /// Turns a FIDL rule table request into the extension type.
    fn into_rule_table_request(
        request: flex_client::fidl::Request<Self::RuleTableMarker>,
    ) -> RuleTableRequest<Self>;
}

impl FidlRuleAdminIpExt for Ipv4 {
    type RuleTableMarker = fnet_routes_admin::RuleTableV4Marker;
    type RuleSetMarker = fnet_routes_admin::RuleSetV4Marker;
    type RuleTableRequestStream = fnet_routes_admin::RuleTableV4RequestStream;
    type RuleSetRequestStream = fnet_routes_admin::RuleSetV4RequestStream;
    type RuleSetAddRuleResponder = fnet_routes_admin::RuleSetV4AddRuleResponder;
    type RuleSetRemoveRuleResponder = fnet_routes_admin::RuleSetV4RemoveRuleResponder;
    type RuleSetAuthenticateForRouteTableResponder =
        fnet_routes_admin::RuleSetV4AuthenticateForRouteTableResponder;
    type RuleTableControlHandle = fnet_routes_admin::RuleTableV4ControlHandle;
    type RuleSetControlHandle = fnet_routes_admin::RuleSetV4ControlHandle;

    fn into_rule_set_request(
        request: flex_client::fidl::Request<Self::RuleSetMarker>,
    ) -> RuleSetRequest<Self> {
        RuleSetRequest::from(request)
    }

    fn into_rule_table_request(
        request: flex_client::fidl::Request<Self::RuleTableMarker>,
    ) -> RuleTableRequest<Self> {
        RuleTableRequest::from(request)
    }
}

impl FidlRuleAdminIpExt for Ipv6 {
    type RuleTableMarker = fnet_routes_admin::RuleTableV6Marker;
    type RuleSetMarker = fnet_routes_admin::RuleSetV6Marker;
    type RuleTableRequestStream = fnet_routes_admin::RuleTableV6RequestStream;
    type RuleSetRequestStream = fnet_routes_admin::RuleSetV6RequestStream;
    type RuleSetAddRuleResponder = fnet_routes_admin::RuleSetV6AddRuleResponder;
    type RuleSetRemoveRuleResponder = fnet_routes_admin::RuleSetV6RemoveRuleResponder;
    type RuleSetAuthenticateForRouteTableResponder =
        fnet_routes_admin::RuleSetV6AuthenticateForRouteTableResponder;
    type RuleTableControlHandle = fnet_routes_admin::RuleTableV6ControlHandle;
    type RuleSetControlHandle = fnet_routes_admin::RuleSetV6ControlHandle;

    fn into_rule_set_request(
        request: flex_client::fidl::Request<Self::RuleSetMarker>,
    ) -> RuleSetRequest<Self> {
        RuleSetRequest::from(request)
    }

    fn into_rule_table_request(
        request: flex_client::fidl::Request<Self::RuleTableMarker>,
    ) -> RuleTableRequest<Self> {
        RuleTableRequest::from(request)
    }
}

impl_responder!(
    fnet_routes_admin::RuleSetV4AddRuleResponder,
    Result<(), fnet_routes_admin::RuleSetError>,
);
impl_responder!(
    fnet_routes_admin::RuleSetV4RemoveRuleResponder,
    Result<(), fnet_routes_admin::RuleSetError>,
);
impl_responder!(
    fnet_routes_admin::RuleSetV4AuthenticateForRouteTableResponder,
    Result<(), fnet_routes_admin::AuthenticateForRouteTableError>,
);
impl_responder!(
    fnet_routes_admin::RuleSetV6AddRuleResponder,
    Result<(), fnet_routes_admin::RuleSetError>,
);
impl_responder!(
    fnet_routes_admin::RuleSetV6RemoveRuleResponder,
    Result<(), fnet_routes_admin::RuleSetError>,
);
impl_responder!(
    fnet_routes_admin::RuleSetV6AuthenticateForRouteTableResponder,
    Result<(), fnet_routes_admin::AuthenticateForRouteTableError>,
);

/// Conversion error for rule elements.
#[derive(Debug, Error, PartialEq)]
pub enum RuleFidlConversionError {
    /// Destination Subnet conversion failed.
    #[error("failed to convert `destination` to net_types subnet: {0:?}")]
    DestinationSubnet(net_types::ip::SubnetError),
    /// Bound interface conversion failed.
    #[error("failed to convert `bound_device`: {0:?}")]
    Interface(fnet_matchers_ext::InterfaceError),
    /// Unknown union variant.
    #[error("unexpected union variant for {name}, got ordinal = ({unknown_ordinal})")]
    #[allow(missing_docs)]
    UnknownOrdinal { name: &'static str, unknown_ordinal: u64 },
}

impl From<fnet_matchers_ext::BoundInterfaceError> for RuleFidlConversionError {
    fn from(value: fnet_matchers_ext::BoundInterfaceError) -> Self {
        match value {
            fnet_matchers_ext::BoundInterfaceError::UnknownUnionVariant(unknown_ordinal) => {
                RuleFidlConversionError::UnknownOrdinal { name: "BoundInterface", unknown_ordinal }
            }
            fnet_matchers_ext::BoundInterfaceError::Interface(interface) => {
                RuleFidlConversionError::Interface(interface)
            }
        }
    }
}

impl From<fnet_matchers_ext::MarkError> for RuleFidlConversionError {
    fn from(value: fnet_matchers_ext::MarkError) -> Self {
        match value {
            fnet_matchers_ext::MarkError::UnknownUnionVariant(unknown_ordinal) => {
                RuleFidlConversionError::UnknownOrdinal { name: "Mark", unknown_ordinal }
            }
        }
    }
}

/// The priority of the rule set, all rule sets are linearized based on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleSetPriority(u32);

impl RuleSetPriority {
    /// Create a new [`RuleSetPriority`].
    pub const fn new(x: u32) -> Self {
        Self(x)
    }
}

/// The priority for the default rule set, where the default rule that points
/// to the main table lives.
pub const DEFAULT_RULE_SET_PRIORITY: RuleSetPriority =
    RuleSetPriority(fnet_routes::DEFAULT_RULE_SET_PRIORITY);

/// The index of a rule within a provided rule set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleIndex(u32);

impl RuleIndex {
    /// Create a new rule index from a scalar.
    pub const fn new(x: u32) -> Self {
        Self(x)
    }
}

impl From<RuleSetPriority> for u32 {
    fn from(RuleSetPriority(x): RuleSetPriority) -> Self {
        x
    }
}

impl From<u32> for RuleSetPriority {
    fn from(x: u32) -> Self {
        Self(x)
    }
}

impl From<RuleIndex> for u32 {
    fn from(RuleIndex(x): RuleIndex) -> Self {
        x
    }
}

impl From<u32> for RuleIndex {
    fn from(x: u32) -> Self {
        Self(x)
    }
}

/// The matcher part of the rule that is used to match packets.
///
/// The default matcher is the one that matches every packets, i.e., all the
/// fields are none.
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash)]
pub struct RuleMatcher<I: Ip> {
    /// Matches whether the source address of the packet is from the subnet.
    pub from: Option<Subnet<I::Addr>>,
    /// Matches the packet iff the packet was locally generated.
    pub locally_generated: Option<bool>,
    /// Matches the packet iff the socket that was bound to the device using
    /// `SO_BINDTODEVICE`.
    pub bound_device: Option<fnet_matchers_ext::BoundInterface>,
    /// The matcher for the MARK_1 domain.
    pub mark_1: Option<fnet_matchers_ext::Mark>,
    /// The matcher for the MARK_2 domain.
    pub mark_2: Option<fnet_matchers_ext::Mark>,
}

impl TryFrom<fnet_routes::RuleMatcherV4> for RuleMatcher<Ipv4> {
    type Error = RuleFidlConversionError;
    fn try_from(
        fnet_routes::RuleMatcherV4 {
            from,
            base,
            __source_breaking: fidl::marker::SourceBreaking,
        }: fnet_routes::RuleMatcherV4,
    ) -> Result<Self, Self::Error> {
        let fnet_routes::BaseMatcher {
            locally_generated,
            bound_device,
            mark_1,
            mark_2,
            __source_breaking: fidl::marker::SourceBreaking,
        } = base.unwrap_or_default();
        Ok(Self {
            from: from
                .map(|from| from.try_into_ext().map_err(RuleFidlConversionError::DestinationSubnet))
                .transpose()?,
            locally_generated,
            bound_device: bound_device
                .map(fnet_matchers_ext::BoundInterface::try_from)
                .transpose()?,
            mark_1: mark_1.map(fnet_matchers_ext::Mark::try_from).transpose()?,
            mark_2: mark_2.map(fnet_matchers_ext::Mark::try_from).transpose()?,
        })
    }
}

impl From<RuleMatcher<Ipv4>> for fnet_routes::RuleMatcherV4 {
    fn from(
        RuleMatcher { from, locally_generated, bound_device, mark_1, mark_2 }: RuleMatcher<Ipv4>,
    ) -> Self {
        let base = fnet_routes::BaseMatcher {
            locally_generated,
            bound_device: bound_device.map(fnet_matchers::BoundInterface::from),
            mark_1: mark_1.map(Into::into),
            mark_2: mark_2.map(Into::into),
            __source_breaking: fidl::marker::SourceBreaking,
        };
        let base = (base != Default::default()).then_some(base);
        fnet_routes::RuleMatcherV4 {
            from: from.map(|from| fnet::Ipv4AddressWithPrefix {
                addr: from.network().into_ext(),
                prefix_len: from.prefix(),
            }),
            base,
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

impl TryFrom<fnet_routes::RuleMatcherV6> for RuleMatcher<Ipv6> {
    type Error = RuleFidlConversionError;
    fn try_from(
        fnet_routes::RuleMatcherV6 {
            from,
            base,
            __source_breaking: fidl::marker::SourceBreaking,
        }: fnet_routes::RuleMatcherV6,
    ) -> Result<Self, Self::Error> {
        let fnet_routes::BaseMatcher {
            locally_generated,
            bound_device,
            mark_1,
            mark_2,
            __source_breaking: fidl::marker::SourceBreaking,
        } = base.unwrap_or_default();
        Ok(Self {
            from: from
                .map(|from| from.try_into_ext().map_err(RuleFidlConversionError::DestinationSubnet))
                .transpose()?,
            locally_generated,
            bound_device: bound_device
                .map(fnet_matchers_ext::BoundInterface::try_from)
                .transpose()?,
            mark_1: mark_1.map(fnet_matchers_ext::Mark::try_from).transpose()?,
            mark_2: mark_2.map(fnet_matchers_ext::Mark::try_from).transpose()?,
        })
    }
}

impl From<RuleMatcher<Ipv6>> for fnet_routes::RuleMatcherV6 {
    fn from(
        RuleMatcher { from, locally_generated, bound_device, mark_1, mark_2 }: RuleMatcher<Ipv6>,
    ) -> Self {
        let base = fnet_routes::BaseMatcher {
            locally_generated,
            bound_device: bound_device.map(fnet_matchers::BoundInterface::from),
            mark_1: mark_1.map(Into::into),
            mark_2: mark_2.map(Into::into),
            __source_breaking: fidl::marker::SourceBreaking,
        };
        let base = (base != Default::default()).then_some(base);
        fnet_routes::RuleMatcherV6 {
            from: from.map(|from| fnet::Ipv6AddressWithPrefix {
                addr: from.network().into_ext(),
                prefix_len: from.prefix(),
            }),
            base,
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
/// Actions of a rule if the matcher matches.
pub enum RuleAction {
    /// Return network is unreachable.
    Unreachable,
    /// Look for a route in the indicated route table. If there is no matching
    /// route in the target table, the lookup will continue to consider the
    /// next rule.
    Lookup(crate::TableId),
}

impl From<fnet_routes::RuleAction> for RuleAction {
    fn from(action: fnet_routes::RuleAction) -> Self {
        match action {
            fnet_routes::RuleAction::Lookup(table_id) => {
                RuleAction::Lookup(crate::TableId::new(table_id))
            }
            fnet_routes::RuleAction::Unreachable(fnet_routes::Unreachable) => {
                RuleAction::Unreachable
            }
            fnet_routes::RuleAction::__SourceBreaking { unknown_ordinal } => {
                panic!("unexpected mark matcher variant, unknown ordinal: {unknown_ordinal}")
            }
        }
    }
}

impl From<RuleAction> for fnet_routes::RuleAction {
    fn from(action: RuleAction) -> Self {
        match action {
            RuleAction::Unreachable => {
                fnet_routes::RuleAction::Unreachable(fnet_routes::Unreachable)
            }
            RuleAction::Lookup(table_id) => fnet_routes::RuleAction::Lookup(table_id.get()),
        }
    }
}

/// GenericOverIp version of RouteTableV{4, 6}Request.
#[derive(GenericOverIp, Debug)]
#[generic_over_ip(I, Ip)]
pub enum RuleTableRequest<I: FidlRuleAdminIpExt> {
    /// Creates a new rule set for the global rule table.
    NewRuleSet {
        /// The priority of the the rule set.
        priority: RuleSetPriority,
        /// The server end of the rule set protocol.
        rule_set: flex_client::fidl::ServerEnd<I::RuleSetMarker>,
        /// Control handle to the protocol.
        control_handle: I::RuleTableControlHandle,
    },
}

impl From<fnet_routes_admin::RuleTableV4Request> for RuleTableRequest<Ipv4> {
    fn from(value: fnet_routes_admin::RuleTableV4Request) -> Self {
        match value {
            fnet_routes_admin::RuleTableV4Request::NewRuleSet {
                priority,
                rule_set,
                control_handle,
            } => Self::NewRuleSet { priority: RuleSetPriority(priority), rule_set, control_handle },
        }
    }
}

impl From<fnet_routes_admin::RuleTableV6Request> for RuleTableRequest<Ipv6> {
    fn from(value: fnet_routes_admin::RuleTableV6Request) -> Self {
        match value {
            fnet_routes_admin::RuleTableV6Request::NewRuleSet {
                priority,
                rule_set,
                control_handle,
            } => Self::NewRuleSet { priority: RuleSetPriority(priority), rule_set, control_handle },
        }
    }
}

/// GenericOverIp version of RuleSetV{4, 6}Request.
#[derive(GenericOverIp, Debug)]
#[generic_over_ip(I, Ip)]
pub enum RuleSetRequest<I: FidlRuleAdminIpExt> {
    /// Adds a rule to the rule set.
    AddRule {
        /// The index of the rule to be added.
        index: RuleIndex,
        /// The matcher of the rule.
        matcher: Result<RuleMatcher<I>, RuleFidlConversionError>,
        /// The action of the rule.
        action: RuleAction,
        /// The responder for this request.
        responder: I::RuleSetAddRuleResponder,
    },
    /// Removes a rule from the rule set.
    RemoveRule {
        /// The index of the rule to be removed.
        index: RuleIndex,
        /// The responder for this request.
        responder: I::RuleSetRemoveRuleResponder,
    },
    /// Authenticates the rule set for managing routes on a route table.
    AuthenticateForRouteTable {
        /// The table id of the table being authenticated for.
        table: u32,
        /// The credential proving authorization for this route table.
        token: flex_client::Event,
        /// The responder for this request.
        responder: I::RuleSetAuthenticateForRouteTableResponder,
    },
    /// Closes the rule set
    Close {
        /// The control handle to rule set protocol.
        control_handle: I::RuleSetControlHandle,
    },
}

impl From<fnet_routes_admin::RuleSetV4Request> for RuleSetRequest<Ipv4> {
    fn from(value: fnet_routes_admin::RuleSetV4Request) -> Self {
        match value {
            fnet_routes_admin::RuleSetV4Request::AddRule { index, matcher, action, responder } => {
                RuleSetRequest::AddRule {
                    index: RuleIndex(index),
                    matcher: matcher.try_into(),
                    action: action.into(),
                    responder,
                }
            }
            fnet_routes_admin::RuleSetV4Request::RemoveRule { index, responder } => {
                RuleSetRequest::RemoveRule { index: RuleIndex(index), responder }
            }
            fnet_routes_admin::RuleSetV4Request::AuthenticateForRouteTable {
                table,
                token,
                responder,
            } => RuleSetRequest::AuthenticateForRouteTable { table, token, responder },
            fnet_routes_admin::RuleSetV4Request::Close { control_handle } => {
                RuleSetRequest::Close { control_handle }
            }
        }
    }
}
impl From<fnet_routes_admin::RuleSetV6Request> for RuleSetRequest<Ipv6> {
    fn from(value: fnet_routes_admin::RuleSetV6Request) -> Self {
        match value {
            fnet_routes_admin::RuleSetV6Request::AddRule { index, matcher, action, responder } => {
                RuleSetRequest::AddRule {
                    index: RuleIndex(index),
                    matcher: matcher.try_into(),
                    action: action.into(),
                    responder,
                }
            }
            fnet_routes_admin::RuleSetV6Request::RemoveRule { index, responder } => {
                RuleSetRequest::RemoveRule { index: RuleIndex(index), responder }
            }
            fnet_routes_admin::RuleSetV6Request::AuthenticateForRouteTable {
                table,
                token,
                responder,
            } => RuleSetRequest::AuthenticateForRouteTable { table, token, responder },
            fnet_routes_admin::RuleSetV6Request::Close { control_handle } => {
                RuleSetRequest::Close { control_handle }
            }
        }
    }
}

/// Rule set creation errors.
#[derive(Clone, Debug, Error)]
pub enum RuleSetCreationError {
    /// Proxy creation failed.
    #[error("failed to create proxy: {0}")]
    CreateProxy(fidl::Error),
    /// Rule set creation failed.
    #[error("failed to create route set: {0}")]
    RuleSet(fidl::Error),
}

/// Creates a new rule set for the rule table.
pub fn new_rule_set<I: Ip + FidlRuleAdminIpExt>(
    rule_table_proxy: &<I::RuleTableMarker as ProtocolMarker>::Proxy,
    priority: RuleSetPriority,
) -> Result<<I::RuleSetMarker as ProtocolMarker>::Proxy, RuleSetCreationError> {
    let (rule_set_proxy, rule_set_server_end) =
        rule_table_proxy.domain().create_proxy::<I::RuleSetMarker>();

    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct NewRuleSetInput<'a, I: FidlRuleAdminIpExt> {
        rule_set_server_end: flex_client::fidl::ServerEnd<I::RuleSetMarker>,
        rule_table_proxy: &'a <I::RuleTableMarker as ProtocolMarker>::Proxy,
    }
    let result = I::map_ip_in(
        NewRuleSetInput::<'_, I> { rule_set_server_end, rule_table_proxy },
        |NewRuleSetInput { rule_set_server_end, rule_table_proxy }| {
            rule_table_proxy.new_rule_set(priority.into(), rule_set_server_end)
        },
        |NewRuleSetInput { rule_set_server_end, rule_table_proxy }| {
            rule_table_proxy.new_rule_set(priority.into(), rule_set_server_end)
        },
    );

    result.map_err(RuleSetCreationError::RuleSet)?;
    Ok(rule_set_proxy)
}

/// Dispatches `authenticate_for_route_table` on either the `RuleSetV4` or
/// `RuleSetV6` proxy.
pub async fn authenticate_for_route_table<I: Ip + FidlRuleAdminIpExt>(
    rule_set: &<I::RuleSetMarker as ProtocolMarker>::Proxy,
    table_id: u32,
    token: flex_client::Event,
) -> Result<Result<(), fnet_routes_admin::AuthenticateForRouteTableError>, fidl::Error> {
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct AuthenticateForRouteTableInput<'a, I: FidlRuleAdminIpExt> {
        rule_set: &'a <I::RuleSetMarker as ProtocolMarker>::Proxy,
        table_id: u32,
        token: flex_client::Event,
    }

    I::map_ip_in(
        AuthenticateForRouteTableInput { rule_set, table_id, token },
        |AuthenticateForRouteTableInput { rule_set, table_id, token }| {
            Either::Left(rule_set.authenticate_for_route_table(table_id, token))
        },
        |AuthenticateForRouteTableInput { rule_set, table_id, token }| {
            Either::Right(rule_set.authenticate_for_route_table(table_id, token))
        },
    )
    .await
}

/// Dispatches `add_rule` on either the `RuleSetV4` or `RuleSetV6` proxy.
pub async fn add_rule<I: Ip + FidlRuleAdminIpExt>(
    rule_set: &<I::RuleSetMarker as ProtocolMarker>::Proxy,
    index: RuleIndex,
    matcher: RuleMatcher<I>,
    action: RuleAction,
) -> Result<Result<(), fnet_routes_admin::RuleSetError>, fidl::Error> {
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct AddRuleInput<'a, I: FidlRuleAdminIpExt> {
        rule_set: &'a <I::RuleSetMarker as ProtocolMarker>::Proxy,
        index: RuleIndex,
        matcher: RuleMatcher<I>,
        action: RuleAction,
    }

    I::map_ip_in(
        AddRuleInput { rule_set, index, matcher, action },
        |AddRuleInput { rule_set, index, matcher, action }| {
            Either::Left(rule_set.add_rule(index.into(), &matcher.into(), &action.into()))
        },
        |AddRuleInput { rule_set, index, matcher, action }| {
            Either::Right(rule_set.add_rule(index.into(), &matcher.into(), &action.into()))
        },
    )
    .await
}

/// Dispatches `remove_rule` on either the `RuleSetV4` or `RuleSetV6` proxy.
pub async fn remove_rule<I: Ip + FidlRuleAdminIpExt>(
    rule_set: &<I::RuleSetMarker as ProtocolMarker>::Proxy,
    index: RuleIndex,
) -> Result<Result<(), fnet_routes_admin::RuleSetError>, fidl::Error> {
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct RemoveRuleInput<'a, I: FidlRuleAdminIpExt> {
        rule_set: &'a <I::RuleSetMarker as ProtocolMarker>::Proxy,
        index: RuleIndex,
    }

    I::map_ip_in(
        RemoveRuleInput { rule_set, index },
        |RemoveRuleInput { rule_set, index }| Either::Left(rule_set.remove_rule(index.into())),
        |RemoveRuleInput { rule_set, index }| Either::Right(rule_set.remove_rule(index.into())),
    )
    .await
}

/// Dispatches `close` on either the `RuleSetV4` or `RuleSetV6` proxy.
///
/// Waits until the channel is closed before returning.
pub async fn close_rule_set<I: Ip + FidlRuleAdminIpExt>(
    rule_set: <I::RuleSetMarker as ProtocolMarker>::Proxy,
) -> Result<(), fidl::Error> {
    use flex_client::fidl::Proxy as _;
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct CloseInput<'a, I: FidlRuleAdminIpExt> {
        rule_set: &'a <I::RuleSetMarker as ProtocolMarker>::Proxy,
    }

    let result = I::map_ip_in(
        CloseInput { rule_set: &rule_set },
        |CloseInput { rule_set }| rule_set.close(),
        |CloseInput { rule_set }| rule_set.close(),
    );

    assert!(
        rule_set
            .on_closed()
            .await
            .expect("failed to wait for signals")
            .contains(fidl::Signals::CHANNEL_PEER_CLOSED)
    );

    result
}

/// Dispatches either `GetRuleWatcherV4` or `GetRuleWatcherV6` on the state proxy.
pub fn get_rule_watcher<I: FidlRuleIpExt + FidlRouteIpExt>(
    state_proxy: &<I::StateMarker as flex_client::fidl::ProtocolMarker>::Proxy,
) -> Result<<I::RuleWatcherMarker as flex_client::fidl::ProtocolMarker>::Proxy, WatcherCreationError>
{
    let (watcher_proxy, watcher_server_end) =
        state_proxy.domain().create_proxy::<I::RuleWatcherMarker>();

    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct GetWatcherInputs<'a, I: FidlRuleIpExt + FidlRouteIpExt> {
        watcher_server_end: flex_client::fidl::ServerEnd<I::RuleWatcherMarker>,
        state_proxy: &'a <I::StateMarker as flex_client::fidl::ProtocolMarker>::Proxy,
    }
    let result = I::map_ip_in(
        GetWatcherInputs::<'_, I> { watcher_server_end, state_proxy },
        |GetWatcherInputs { watcher_server_end, state_proxy }| {
            state_proxy.get_rule_watcher_v4(
                watcher_server_end,
                &fnet_routes::RuleWatcherOptionsV4::default(),
            )
        },
        |GetWatcherInputs { watcher_server_end, state_proxy }| {
            state_proxy.get_rule_watcher_v6(
                watcher_server_end,
                &fnet_routes::RuleWatcherOptionsV6::default(),
            )
        },
    );

    result.map_err(WatcherCreationError::GetWatcher)?;
    Ok(watcher_proxy)
}

/// Calls `Watch()` on the provided `RuleWatcherV4` or `RuleWatcherV6` proxy.
pub async fn watch<'a, I: FidlRuleIpExt>(
    watcher_proxy: &'a <I::RuleWatcherMarker as flex_client::fidl::ProtocolMarker>::Proxy,
) -> Result<Vec<I::RuleEvent>, fidl::Error> {
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct WatchInputs<'a, I: FidlRuleIpExt> {
        watcher_proxy: &'a <I::RuleWatcherMarker as flex_client::fidl::ProtocolMarker>::Proxy,
    }
    #[derive(GenericOverIp)]
    #[generic_over_ip(I, Ip)]
    struct WatchOutputs<I: FidlRuleIpExt> {
        watch_fut: fidl::client::QueryResponseFut<Vec<I::RuleEvent>, flex_client::Dialect>,
    }
    let WatchOutputs { watch_fut } = net_types::map_ip_twice!(
        I,
        WatchInputs { watcher_proxy },
        |WatchInputs { watcher_proxy }| { WatchOutputs { watch_fut: watcher_proxy.watch() } }
    );
    watch_fut.await
}

/// Route watcher `Watch` errors.
#[derive(Debug, Error)]
pub enum RuleWatchError {
    /// The call to `Watch` returned a FIDL error.
    #[error("the call to `Watch()` failed: {0}")]
    Fidl(fidl::Error),
    /// The event returned by `Watch` encountered a conversion error.
    #[error("failed to convert event returned by `Watch()`: {0}")]
    Conversion(RuleFidlConversionError),
    /// The server returned an empty batch of events.
    #[error("the call to `Watch()` returned an empty batch of events")]
    EmptyEventBatch,
}

/// Creates a rules event stream from the state proxy.
pub fn rule_event_stream_from_state<I: FidlRuleIpExt + FidlRouteIpExt>(
    state: &<I::StateMarker as flex_client::fidl::ProtocolMarker>::Proxy,
) -> Result<impl Stream<Item = Result<RuleEvent<I>, RuleWatchError>> + use<I>, WatcherCreationError>
{
    let watcher = get_rule_watcher::<I>(state)?;
    rule_event_stream_from_watcher(watcher)
}

/// Turns the provided watcher client into a [`RuleEvent`] stream by applying
/// Hanging-Get watch.
///
/// Each call to `Watch` returns a batch of events, which are flattened into a
/// single stream. If an error is encountered while calling `Watch` or while
/// converting the event, the stream is immediately terminated.
pub fn rule_event_stream_from_watcher<I: FidlRuleIpExt>(
    watcher: <I::RuleWatcherMarker as flex_client::fidl::ProtocolMarker>::Proxy,
) -> Result<impl Stream<Item = Result<RuleEvent<I>, RuleWatchError>> + use<I>, WatcherCreationError>
{
    Ok(stream::ShortCircuit::new(
        futures::stream::try_unfold(watcher, |watcher| async {
            let events_batch = watch::<I>(&watcher).await.map_err(RuleWatchError::Fidl)?;
            if events_batch.is_empty() {
                return Err(RuleWatchError::EmptyEventBatch);
            }
            let events_batch = events_batch
                .into_iter()
                .map(|event| event.try_into().map_err(RuleWatchError::Conversion));
            let event_stream = futures::stream::iter(events_batch);
            Ok(Some((event_stream, watcher)))
        })
        // Flatten the stream of event streams into a single event stream.
        .try_flatten(),
    ))
}

/// Errors returned by [`collect_rules_until_idle`].
#[derive(Debug, Error)]
pub enum CollectRulesUntilIdleError<I: FidlRuleIpExt> {
    /// There was an error in the event stream.
    #[error("there was an error in the event stream: {0}")]
    ErrorInStream(RuleWatchError),
    /// There was an unexpected event in the event stream. Only `existing` or
    /// `idle` events are expected.
    #[error("there was an unexpected event in the event stream: {0:?}")]
    UnexpectedEvent(RuleEvent<I>),
    /// The event stream unexpectedly ended.
    #[error("the event stream unexpectedly ended")]
    StreamEnded,
}

/// Collects all `existing` events from the stream, stopping once the `idle`
/// event is observed.
pub async fn collect_rules_until_idle<I: FidlRuleIpExt, C: Extend<InstalledRule<I>> + Default>(
    event_stream: impl futures::Stream<Item = Result<RuleEvent<I>, RuleWatchError>> + Unpin,
) -> Result<C, CollectRulesUntilIdleError<I>> {
    fold::fold_while(
        event_stream,
        Ok(C::default()),
        |existing_rules: Result<C, CollectRulesUntilIdleError<I>>, event| {
            futures::future::ready(match existing_rules {
                Err(_) => {
                    unreachable!("`existing_rules` must be `Ok`, because we stop folding on err")
                }
                Ok(mut existing_rules) => match event {
                    Err(e) => {
                        fold::FoldWhile::Done(Err(CollectRulesUntilIdleError::ErrorInStream(e)))
                    }
                    Ok(e) => match e {
                        RuleEvent::Existing(e) => {
                            existing_rules.extend([e]);
                            fold::FoldWhile::Continue(Ok(existing_rules))
                        }
                        RuleEvent::Idle => fold::FoldWhile::Done(Ok(existing_rules)),
                        e @ RuleEvent::Added(_) | e @ RuleEvent::Removed(_) => {
                            fold::FoldWhile::Done(Err(CollectRulesUntilIdleError::UnexpectedEvent(
                                e,
                            )))
                        }
                    },
                },
            })
        },
    )
    .await
    .short_circuited()
    .map_err(|_accumulated_thus_far: Result<C, CollectRulesUntilIdleError<I>>| {
        CollectRulesUntilIdleError::StreamEnded
    })?
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;

    #[test]
    fn missing_base_matcher_default_v4() {
        let fidl_matcher = flex_fuchsia_net_routes::RuleMatcherV4 {
            from: None,
            base: None,
            __source_breaking: fidl::marker::SourceBreaking,
        };
        assert_eq!(RuleMatcher::try_from(fidl_matcher), Ok(Default::default()));
    }

    #[test]
    fn missing_base_matcher_default_v6() {
        let fidl_matcher = flex_fuchsia_net_routes::RuleMatcherV6 {
            from: None,
            base: None,
            __source_breaking: fidl::marker::SourceBreaking,
        };
        assert_eq!(RuleMatcher::try_from(fidl_matcher), Ok(Default::default()));
    }

    #[test]
    fn invalid_destination_subnet_v4() {
        let fidl_matcher = flex_fuchsia_net_routes::RuleMatcherV4 {
            // Invalid, because subnets should not have the "host bits" set.
            from: Some(net_declare::fidl_ip_v4_with_prefix!("192.168.0.1/24")),
            base: None,
            __source_breaking: fidl::marker::SourceBreaking,
        };
        assert_matches!(
            RuleMatcher::try_from(fidl_matcher),
            Err(RuleFidlConversionError::DestinationSubnet(_))
        );
    }

    #[test]
    fn invalid_destination_subnet_v6() {
        let fidl_matcher = flex_fuchsia_net_routes::RuleMatcherV6 {
            // Invalid, because subnets should not have the "host bits" set.
            from: Some(net_declare::fidl_ip_v6_with_prefix!("fe80::1/64")),
            base: None,
            __source_breaking: fidl::marker::SourceBreaking,
        };
        assert_matches!(
            RuleMatcher::try_from(fidl_matcher),
            Err(RuleFidlConversionError::DestinationSubnet(_))
        );
    }

    #[test]
    fn all_unspecified_matcher_v4() {
        let fidl_matcher = flex_fuchsia_net_routes::RuleMatcherV4 {
            from: None,
            base: None,
            __source_breaking: fidl::marker::SourceBreaking,
        };
        let ext_matcher = RuleMatcher {
            from: None,
            locally_generated: None,
            bound_device: None,
            mark_1: None,
            mark_2: None,
        };
        assert_eq!(RuleMatcher::try_from(fidl_matcher.clone()), Ok(ext_matcher.clone()));
        assert_eq!(flex_fuchsia_net_routes::RuleMatcherV4::from(ext_matcher), fidl_matcher,)
    }

    #[test]
    fn all_unspecified_matcher_v6() {
        let fidl_matcher = flex_fuchsia_net_routes::RuleMatcherV6 {
            from: None,
            base: None,
            __source_breaking: fidl::marker::SourceBreaking,
        };
        let ext_matcher = RuleMatcher {
            from: None,
            locally_generated: None,
            bound_device: None,
            mark_1: None,
            mark_2: None,
        };
        assert_eq!(RuleMatcher::try_from(fidl_matcher.clone()), Ok(ext_matcher.clone()));
        assert_eq!(flex_fuchsia_net_routes::RuleMatcherV6::from(ext_matcher), fidl_matcher,)
    }
}
