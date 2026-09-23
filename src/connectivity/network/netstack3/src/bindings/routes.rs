// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Defines the types of changes that can be made to the routing table, and the
//! worker responsible for executing those changes.
//!
//! Routing table changes are requested via an mpsc Sender held in BindingsCtx
//! ([`Changes`]), while the [`ChangeRunner`] is run in a separate task and is
//! responsible for ingesting those changes, updating the routing table, and
//! syncing the table to core.
//!
//! This is the source of truth for the netstack routing table, and the routing
//! table in core should be viewed as downstream of this one. This allows
//! bindings to implement routing table features without needing core to know
//! about them, such as the reference-counted RouteSets specified in
//! fuchsia.net.routes.admin.

use std::borrow::{Borrow as _, Cow};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::num::NonZeroUsize;
use std::sync::Arc;

use assert_matches::assert_matches;
use derivative::Derivative;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
use fidl_fuchsia_net_routes_ext::admin::FidlRouteAdminIpExt;
use fidl_fuchsia_net_routes_ext::rules::{
    DEFAULT_RULE_SET_PRIORITY, InstalledRule, RuleAction, RuleEvent, RuleIndex, RuleSetPriority,
};
use futures::channel::{mpsc, oneshot};
use futures::{Future, FutureExt as _, StreamExt as _, stream};
use log::{debug, error, info, warn};
use net_types::SpecifiedAddr;
use net_types::ip::{GenericOverIp, Ip, IpAddress, IpVersion, Ipv4, Ipv6, Subnet};
use netstack3_core::routes::{AddableEntry, AddableMetric};
use thiserror::Error;

use crate::bindings::util::{
    EntryAndTableId, RemoveResourceResultExt as _, ResultExt as _, TryIntoFidlWithContext,
};
use crate::bindings::{BindingsCtx, Ctx, DeviceIdExt, IpExt};

pub(crate) mod admin;
pub(crate) mod interface_local;
pub(crate) mod rules_admin;
mod rules_state;
use admin::{StrongUserRouteSet, WeakUserRouteSet};
use rules_admin::{NewRuleSet, RuleOp, RuleTable, RuleWorkItem, SetPriorityConflict};

pub(crate) mod state;
pub(crate) mod watcher;
mod witness;
pub(crate) use witness::{TableId, TableIdEither, main_table_id};

type WeakDeviceId = netstack3_core::device::WeakDeviceId<crate::bindings::BindingsCtx>;
type DeviceId = netstack3_core::device::DeviceId<crate::bindings::BindingsCtx>;

#[derive(GenericOverIp, Debug, Clone)]
#[generic_over_ip(T, GenericOverIp)]
pub(crate) enum Matcher<T> {
    /// Only matches when the input exactly matches the specified.
    Exact(T),
    /// Matches any input.
    Any,
}

impl<T: PartialEq> Matcher<T> {
    pub(crate) fn matches(&self, input: &T) -> bool {
        match self {
            Matcher::Exact(matcher) => matcher == input,
            Matcher::Any => true,
        }
    }
}
#[derive(GenericOverIp, Debug)]
#[generic_over_ip(A, IpAddress)]
#[derive(Clone)]
pub(crate) enum RouteOp<A: IpAddress> {
    Add(netstack3_core::routes::AddableEntry<A, WeakDeviceId>),
    RemoveMatching {
        subnet: Subnet<A>,
        device: WeakDeviceId,
        gateway: Matcher<Option<SpecifiedAddr<A>>>,
        metric: Matcher<AddableMetric>,
    },
}

#[derive(Error, Debug)]
#[error("the device has already been removed")]
struct DeviceRemovedError;

impl<A: IpAddress> RouteOp<A> {
    fn get_interface_local_table(&self) -> Result<Option<TableId<A::Version>>, DeviceRemovedError> {
        let device = match self {
            RouteOp::Add(AddableEntry {
                subnet: _,
                device,
                gateway: _,
                metric: _,
                route_preference: _,
            }) => device,
            RouteOp::RemoveMatching { subnet: _, device, gateway: _, metric: _ } => device,
        };
        let strong = device.upgrade().ok_or(DeviceRemovedError)?;
        Ok(strong
            .external_state()
            .static_common_info()
            .local_route_tables
            .as_ref()
            .map(|t| t.get::<A::Version>().table_id))
    }
}

#[derive(GenericOverIp, Debug)]
#[generic_over_ip(I, Ip)]
struct AddTableOutcome<I: Ip> {
    table_id: TableId<I>,
    route_work_sink: mpsc::UnboundedSender<RouteWorkItem<I>>,
    token: Arc<zx::Event>,
}

#[derive(GenericOverIp, Debug)]
#[generic_over_ip(I, Ip)]
struct AddTableOp<I: Ip> {
    name: Option<String>,
    responder: oneshot::Sender<Result<AddTableOutcome<I>, TableIdOverflowsError>>,
}

#[derive(Debug, GenericOverIp)]
#[generic_over_ip(I, Ip)]
struct GetRouteTableNameOp<I: Ip> {
    table_id: TableId<I>,
    responder: fnet_routes::StateGetRouteTableNameResponder,
}

#[derive(GenericOverIp, Debug)]
#[generic_over_ip(I, Ip)]
enum TableOp<I: Ip> {
    AddTable(AddTableOp<I>),
    GetRouteTableName(GetRouteTableNameOp<I>),
}

#[derive(GenericOverIp, Derivative)]
#[generic_over_ip(I, Ip)]
#[derive(Clone)]
#[derivative(Debug)]
pub(crate) enum Change<I: Ip> {
    RouteOp(RouteOp<I::Addr>, WeakSetMembership<I>),
    RemoveSet(#[derivative(Debug = "ignore")] WeakUserRouteSet<I>),
    RemoveMatchingDevice(WeakDeviceId),
    RemoveTable(TableId<I>),
}

pub(crate) enum ChangeEither {
    V4(Change<Ipv4>),
    V6(Change<Ipv6>),
}

impl ChangeEither {
    pub(crate) fn global_add(
        entry: netstack3_core::routes::AddableEntryEither<WeakDeviceId>,
    ) -> Self {
        match entry {
            netstack3_core::routes::AddableEntryEither::V4(entry) => {
                Self::V4(Change::RouteOp(RouteOp::Add(entry), SetMembership::Global))
            }
            netstack3_core::routes::AddableEntryEither::V6(entry) => {
                Self::V6(Change::RouteOp(RouteOp::Add(entry), SetMembership::Global))
            }
        }
    }
}

impl<I: Ip> From<Change<I>> for ChangeEither {
    fn from(change: Change<I>) -> Self {
        I::map_ip_in(change, |change| ChangeEither::V4(change), |change| ChangeEither::V6(change))
    }
}

#[derive(Debug, Error)]
pub(crate) enum ChangeError {
    #[error("route's device no longer exists")]
    DeviceRemoved,
    #[error("route table is removed")]
    TableRemoved,
    #[error("route set no longer exists")]
    SetRemoved,
}

#[derive(Debug)]

pub(crate) struct TableIdOverflowsError;

#[derive(Debug)]
pub(crate) struct RouteWorkItem<I: Ip> {
    pub(crate) change: Change<I>,
    pub(crate) responder: Option<oneshot::Sender<Result<ChangeOutcome, ChangeError>>>,
}

struct TableWorkItem<I: Ip> {
    op: TableOp<I>,
}

/// The routing table from the perspective of bindings.
///
/// This is the source of truth for the netstack's routing table; the core
/// routing table should be viewed as downstream of this one. This allows
/// bindings to implement route-set-membership semantics without requiring
/// the concept of a route set to be implemented in core.
#[derive(Debug, Clone)]
struct Table<I: Ip> {
    inner: HashMap<netstack3_core::routes::AddableEntry<I::Addr, DeviceId>, EntryData<I>>,
    /// The next [`netstack3_core::routes::Generation`] to be applied to new
    /// entries. This allows the routing table ordering to explicitly take into
    /// account the order in which routes are added to the table.
    next_generation: netstack3_core::routes::Generation,
    /// The authentication token used to authorize route rules.
    token: Arc<zx::Event>,
    /// The table id that we can use to update the routing tables in Core.
    core_id: CoreId<I>,
    /// The optional name given to the table for debugging purposes.
    name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialOrd, Ord, PartialEq, Eq, Hash)]
enum TableModifyResult<T> {
    NoChange,
    SetChanged,
    TableChanged(T),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeOutcome {
    NoChange,
    Changed,
}

#[derive(Debug, Clone)]
enum CoreId<I: Ip> {
    Main,
    User(netstack3_core::routes::RoutingTableId<I, DeviceId, BindingsCtx>),
}

impl<I: Ip> Table<I> {
    fn new(
        initial_generation: netstack3_core::routes::Generation,
        core_id: CoreId<I>,
        name: Option<String>,
    ) -> Self {
        let token = Arc::new(zx::Event::create());
        Self { inner: HashMap::new(), next_generation: initial_generation, token, core_id, name }
    }

    fn insert(
        &mut self,
        route: netstack3_core::routes::AddableEntry<I::Addr, DeviceId>,
        set: StrongSetMembership<I>,
    ) -> TableModifyResult<(
        netstack3_core::routes::AddableEntry<I::Addr, DeviceId>,
        netstack3_core::routes::Generation,
    )> {
        let Self { inner, next_generation, token: _, core_id, name: _ } = self;
        let (entry, new_to_table) = match inner.entry(route.clone()) {
            std::collections::hash_map::Entry::Occupied(occupied_entry) => {
                (occupied_entry.into_mut(), false)
            }
            std::collections::hash_map::Entry::Vacant(vacant_entry) => (
                vacant_entry.insert({
                    let generation = *next_generation;
                    *next_generation = next_generation.next();
                    EntryData::new(generation)
                }),
                true,
            ),
        };
        let new_to_set = entry.set_membership.insert(set.downgrade(), set.clone()).is_none();
        let result = if new_to_table {
            TableModifyResult::TableChanged((route.clone(), entry.generation))
        } else if new_to_set {
            TableModifyResult::SetChanged
        } else {
            TableModifyResult::NoChange
        };
        info!("insert route {route:?} (table={core_id:?} set={set:?}) had result {result:?}");
        result
    }

    /// Given a predicate and an indication of the route set to operate on,
    /// removes routes that match the predicate.
    ///
    /// If `set` is `SetMembership::Global`, then routes matching the predicate
    /// are removed from the table regardless of set membership. Otherwise,
    /// routes matching the predicate are removed from the indicated set, and
    /// then only removed from the overall table if that was the last reference
    /// to the route.
    fn remove<F: FnMut(&netstack3_core::routes::AddableEntry<I::Addr, DeviceId>) -> bool>(
        &mut self,
        mut should_remove: F,
        set: WeakSetMembership<I>,
    ) -> TableModifyResult<
        Vec<(
            netstack3_core::routes::AddableEntry<I::Addr, DeviceId>,
            netstack3_core::routes::Generation,
        )>,
    > {
        let Self { inner, next_generation: _, token: _, core_id, name: _ } = self;

        let mut removed_any_from_set = false;
        let mut removed_from_table = Vec::new();

        inner.retain(|route, data| {
            if !should_remove(route) {
                return true;
            }

            let should_remove_from_table = match &set {
                // "Global" removes mean we remove the route from the table
                // regardless of set membership.
                SetMembership::Global => true,
                SetMembership::CoreNdp
                | SetMembership::InitialDeviceRoutes
                | SetMembership::Loopback
                | SetMembership::User(_) => {
                    // Non-global named sets and user sets behave alike.
                    match data.set_membership.remove(&set) {
                        None => {
                            // Was not in the set.
                        }
                        Some(membership) => {
                            // Was in the set, this is the corresponding strong ID.
                            let _: StrongSetMembership<_> = membership;
                            removed_any_from_set = true;
                        }
                    };
                    data.set_membership.is_empty()
                }
            };

            if should_remove_from_table {
                removed_from_table.push((route.clone(), data.generation));
                false
            } else {
                true
            }
        });

        let result = {
            if !removed_from_table.is_empty() {
                info!(
                    "remove operation on routing table (table={core_id:?}) from set {set:?} \
                    resulted in removal of {} routes from the table:",
                    removed_from_table.len()
                );
                for (route, generation) in &removed_from_table {
                    info!(
                        "removed route {route:?} (table={core_id:?} set={set:?} \
                        generation={generation:?})"
                    );
                }
                TableModifyResult::TableChanged(removed_from_table)
            } else if removed_any_from_set {
                info!(
                    "remove operation on routing table (table={core_id:?}) removed routes from set \
                    {set:?}, but not the overall table"
                );
                TableModifyResult::SetChanged
            } else {
                info!(
                    "remove operation on routing table (table={core_id:?}) from set {set:?} \
                    resulted in no change"
                );
                TableModifyResult::NoChange
            }
        };
        result
    }

    fn remove_user_set(
        &mut self,
        set: WeakUserRouteSet<I>,
    ) -> Vec<(
        netstack3_core::routes::AddableEntry<I::Addr, DeviceId>,
        netstack3_core::routes::Generation,
    )> {
        let Self { inner, next_generation: _, token: _, core_id, name: _ } = self;
        let set = SetMembership::User(set);
        let mut removed_from_table = Vec::new();
        inner.retain(|route, data| {
            if data.set_membership.remove(&set).is_some() && data.set_membership.is_empty() {
                removed_from_table.push((route.clone(), data.generation));
                false
            } else {
                true
            }
        });

        info!(
            "route set removal ({set:?}) removed {} routes from table ({core_id:?}):",
            removed_from_table.len()
        );

        for (route, generation) in &removed_from_table {
            info!(
                "removed route {route:?} (table={core_id:?} set={set:?} generation={generation:?})"
            );
        }

        removed_from_table
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Derivative)]
#[derivative(Debug)]
pub(crate) enum SetMembership<T> {
    /// Indicates route changes that are applied globally -- routes added
    /// globally cannot be removed by other route sets, but removing a route
    /// globally will also remove that route from other route sets.
    Global,
    /// Routes added or removed by core due to NDP belong to this route set.
    CoreNdp,
    /// Routes added as part of initial device bringup belong to this route set.
    InitialDeviceRoutes,
    /// Routes added as part of loopback device bringup belong to this route
    /// set.
    Loopback,
    /// Route sets created ephemerally (usually as part of serving FIDL
    /// protocols that involve managing route lifetimes) belong to this class
    /// of route sets.
    User(#[derivative(Debug = "ignore")] T),
}

type StrongSetMembership<I> = SetMembership<StrongUserRouteSet<I>>;
type WeakSetMembership<I> = SetMembership<WeakUserRouteSet<I>>;

impl<I: Ip> StrongSetMembership<I> {
    fn downgrade(&self) -> WeakSetMembership<I> {
        match self {
            SetMembership::Global => SetMembership::Global,
            SetMembership::CoreNdp => SetMembership::CoreNdp,
            SetMembership::InitialDeviceRoutes => SetMembership::InitialDeviceRoutes,
            SetMembership::Loopback => SetMembership::Loopback,
            SetMembership::User(set) => {
                SetMembership::User(netstack3_core::sync::StrongRc::downgrade(&set))
            }
        }
    }
}

impl<I: Ip> WeakSetMembership<I> {
    #[cfg_attr(feature = "instrumented", track_caller)]
    fn upgrade(self) -> Option<StrongSetMembership<I>> {
        match self {
            SetMembership::Global => Some(SetMembership::Global),
            SetMembership::CoreNdp => Some(SetMembership::CoreNdp),
            SetMembership::InitialDeviceRoutes => Some(SetMembership::InitialDeviceRoutes),
            SetMembership::Loopback => Some(SetMembership::Loopback),
            SetMembership::User(set) => set.upgrade().map(SetMembership::User),
        }
    }
}

#[derive(Clone, Debug)]
struct EntryData<I: Ip> {
    generation: netstack3_core::routes::Generation,
    // Logically, this should be viewed as a `HashSet<StrongSetMembership>`, but
    // we use a `HashMap<WeakSetMembership, StrongSetMembership>` (where the
    // key and value set-IDs always match) in order to be able to look up using
    // only a weak set ID. We want to keep strong set memberships in the map
    // so that we can assert that we have cleaned up all references to a user
    // route set by unwrapping the primary route set ID.
    set_membership: HashMap<WeakSetMembership<I>, StrongSetMembership<I>>,
}

impl<I: Ip> EntryData<I> {
    fn new(generation: netstack3_core::routes::Generation) -> Self {
        Self { generation, set_membership: HashMap::new() }
    }
}

type RouteWorkReceivers<I> =
    async_utils::stream::OneOrMany<stream::StreamFuture<mpsc::UnboundedReceiver<RouteWorkItem<I>>>>;

#[derive(Default)]
struct DefaultRoutesByDevice {
    // Maps a device ID to the number of default routes currently installed for that device,
    // across all tables. We key this map on a strong DeviceId because the entry here cannot
    // go away until all default routes associated with the device are removed.
    inner: HashMap<DeviceId, NonZeroUsize>,
}

impl DefaultRoutesByDevice {
    /// Returns the new count of default routes for the device.
    fn add(&mut self, device: DeviceId) -> usize {
        self.inner
            .entry(device)
            .and_modify(|count| *count = count.checked_add(1).unwrap())
            .or_insert_with(|| NonZeroUsize::new(1).unwrap())
            .get()
    }

    /// Returns the new count of default routes for the device.
    ///
    /// Panics if the device is not found in the map.
    fn remove(&mut self, device: DeviceId) -> usize {
        match self.inner.entry(device) {
            Entry::Occupied(mut entry) => {
                let count = entry.get_mut();
                match NonZeroUsize::new(count.get() - 1) {
                    Some(new_count) => {
                        *count = new_count;
                        new_count.get()
                    }
                    None => {
                        let _: NonZeroUsize = entry.remove();
                        0
                    }
                }
            }
            Entry::Vacant(_) => panic!("device not found"),
        }
    }
}

pub(crate) struct State<I: Ip> {
    last_table_id: TableId<I>,
    table_work_receiver: mpsc::UnboundedReceiver<TableWorkItem<I>>,
    route_work_receivers: RouteWorkReceivers<I>,
    new_rule_set_receiver: mpsc::UnboundedReceiver<NewRuleSet<I>>,
    tables: HashMap<TableId<I>, Table<I>>,
    rules: RuleTable<I>,
    dispatchers: Dispatchers<I>,
    default_routes_by_device: DefaultRoutesByDevice,
}

#[derive(Derivative)]
#[derivative(Clone(bound = ""))]
pub(crate) struct Changes<I: Ip> {
    table_work_sink: mpsc::UnboundedSender<TableWorkItem<I>>,
    main_table_route_work_sink: mpsc::UnboundedSender<RouteWorkItem<I>>,
    main_table_token: Arc<zx::Event>,
    new_rule_set_sink: mpsc::UnboundedSender<NewRuleSet<I>>,
}

impl<I: Ip> Changes<I> {
    fn close_senders(&self) {
        let Self {
            table_work_sink,
            main_table_route_work_sink,
            new_rule_set_sink,
            main_table_token: _,
        } = self;
        table_work_sink.close_channel();
        main_table_route_work_sink.close_channel();
        new_rule_set_sink.close_channel();
    }
}

#[netstack3_core::context_ip_bounds(I, BindingsCtx)]
impl<I> State<I>
where
    I: IpExt + FidlRouteAdminIpExt,
{
    pub(crate) async fn run_changes(&mut self, mut ctx: Ctx) {
        let State {
            table_work_receiver,
            route_work_receivers,
            new_rule_set_receiver,
            tables,
            rules,
            dispatchers: Dispatchers { route_update_dispatcher, rule_update_dispatcher },
            last_table_id,
            default_routes_by_device,
        } = self;
        let mut rule_set_work_receivers = stream::FuturesUnordered::new();
        loop {
            futures::select_biased!(
                route_work_item = route_work_receivers.next() => {
                    let Some((Some(route_work_item), mut rest)) = route_work_item else {
                        continue;
                    };
                    let removing =  match route_work_item {
                        RouteWorkItem {
                            change: Change::RemoveTable(table_id),
                            responder: _,
                        } => Some(table_id),
                        RouteWorkItem {
                            change: Change::RouteOp(_, _)
                                | Change::RemoveSet(_)
                                | Change::RemoveMatchingDevice(_),
                            responder: _,
                        } => None,
                    };
                    // Remove all the rules that reference this table.
                    if let Some(table_id) = removing {
                        let removed = rules.handle_table_removed(table_id);
                        Self::install_rules_and_notify_watchers(
                            &mut ctx,
                            rules,
                            tables,
                            removed.into_iter().map(watcher::Update::Removed),
                            rule_update_dispatcher,
                        )
                    }
                    Self::handle_route_change(
                        &mut ctx,
                        tables,
                        route_update_dispatcher,
                        route_work_item,
                        default_routes_by_device,
                    );
                    if let Some(table_id) = removing {
                        let Table { core_id, inner, .. } = tables.remove(&table_id)
                            .expect("invalid table ID");
                        let core_id = assert_matches!(core_id,
                            CoreId::User(core_id) => core_id, "cannot remove main table");
                        info!("Removing Route Table: {core_id:?}");
                        let weak = core_id.downgrade();
                        ctx.api().routes()
                            .remove_table(core_id)
                            .map_deferred(|d| d.into_future("table id", &weak, &ctx))
                            .into_future()
                            .await;
                        // Make sure all the strong references to the route set is gone before
                        // we respond to any of the pending requests. Because among all the
                        // pending requests, there maybe a `RemoveSet` request. Once we respond,
                        // the requesting task will proceed to unwrap the primary ID for the
                        // route set. This will cause panic if there are still strong references.
                        drop(inner);
                        rest.close();
                        rest.filter_map(|RouteWorkItem {
                            change: _,
                            responder,
                        }| futures::future::ready(responder))
                        .for_each(|responder| futures::future::ready(
                            responder.send(Err(ChangeError::TableRemoved)).unwrap_or_else(|err| {
                                error!("failed to respond to the change request: {err:?}");
                            })
                        )).await;
                    } else {
                        route_work_receivers.push(rest.into_future());
                    }
                },
                table_work = table_work_receiver.next() => {
                    let Some(table_work) = table_work else {
                        continue;
                    };
                    Self::handle_table_op(
                        &mut ctx,
                        last_table_id,
                        tables,
                        table_work,
                        route_work_receivers
                    )
                },
                new_rule_set = new_rule_set_receiver.next() => {
                    let Some(NewRuleSet {
                        priority,
                        rule_set_work_receiver,
                        responder
                    }) = new_rule_set else {
                        continue;
                    };
                    let result = rules.new_rule_set(priority);
                    rule_set_work_receivers.push(
                        rule_set_work_receiver.into_future());
                    responder.send(result).expect("failed to send result");
                }
                rule_set_work_item = rule_set_work_receivers.next() => {
                    let Some((Some(rule_work), mut rest)) = rule_set_work_item else {
                        continue;
                    };
                    match rule_work {
                        RuleWorkItem::RuleOp {
                            op, responder
                        } => {
                            let is_removal = matches!(op, RuleOp::RemoveSet { .. });
                            let result = Self::handle_rule_op(
                                &mut ctx,
                                rules,
                                tables,
                                op,
                                rule_update_dispatcher
                            );
                            if is_removal {
                                // We must close the channel so that the rule set
                                // can no longer send RuleOps. Note that we make
                                // sure no more operations will be sent once
                                // `RemoveSet` is sent.
                                rest.close();
                            } else {
                                rule_set_work_receivers.push(rest.into_future());
                            }
                            responder.send(result).expect("the receiver is dropped");
                        }
                        RuleWorkItem::AuthenticateForRouteTable {
                            table_id, token, responder
                        } => {
                            let result = Self::handle_route_table_authentication(
                                tables,
                                table_id,
                                token,
                            );
                            rule_set_work_receivers.push(rest.into_future());
                            responder.send(result).expect("the receiver is dropped");
                        }
                    }
                }
                complete => break,
            )
        }
    }

    fn handle_route_change(
        ctx: &mut Ctx,
        tables: &mut HashMap<TableId<I>, Table<I>>,
        update_dispatcher: &mut state::RouteUpdateDispatcher<I>,
        RouteWorkItem { change, responder }: RouteWorkItem<I>,
        default_routes_by_device: &mut DefaultRoutesByDevice,
    ) {
        let result = handle_route_change::<I>(
            tables,
            ctx,
            change,
            update_dispatcher,
            default_routes_by_device,
        );
        if let Some(responder) = responder {
            match responder.send(result) {
                Ok(()) => (),
                Err(result) => match result {
                    Ok(outcome) => {
                        match outcome {
                            ChangeOutcome::NoChange | ChangeOutcome::Changed => {
                                // We don't need to log anything here;
                                // the change succeeded.
                            }
                        }
                    }
                    Err(e) => {
                        // Since the other end dropped the receiver, no one will
                        // observe the result of this route change, so we have to
                        // log any errors ourselves.
                        error!("error while handling route change: {:?}", e);
                    }
                },
            };
        }
    }

    fn handle_table_op(
        ctx: &mut Ctx,
        last_table_id: &mut TableId<I>,
        tables: &mut HashMap<TableId<I>, Table<I>>,
        TableWorkItem { op }: TableWorkItem<I>,
        route_work_receivers: &mut RouteWorkReceivers<I>,
    ) {
        match op {
            TableOp::AddTable(AddTableOp { name, responder }) => {
                let result = {
                    match last_table_id.next() {
                        // Never reuse table IDs, so the table IDs can only be
                        // increasing.
                        None => Err(TableIdOverflowsError),
                        Some(table_id) => {
                            let core_id = ctx.api().routes().new_table(table_id.into());
                            let core_id = CoreId::User(core_id);
                            info!("Adding Route Table: name={name:?} ({core_id:?})");
                            let new_table = Table::new(
                                netstack3_core::routes::Generation::initial(),
                                core_id,
                                name,
                            );
                            let token = new_table.token.clone();
                            assert_matches!(tables.insert(table_id, new_table), None);
                            *last_table_id = table_id;
                            let (route_work_sink, route_work_receiver) = mpsc::unbounded();
                            route_work_receivers.push(route_work_receiver.into_future());
                            Ok(AddTableOutcome { table_id, route_work_sink, token })
                        }
                    }
                };
                responder.send(result).expect("the receiver should still be alive");
            }
            TableOp::GetRouteTableName(GetRouteTableNameOp { table_id, responder }) => {
                enum Outcome<'a> {
                    Set(&'a String),
                    Unset,
                    NoTable,
                }

                let outcome = tables.get(&table_id).map_or(Outcome::NoTable, |table| {
                    table.name.as_ref().map_or(Outcome::Unset, Outcome::Set)
                });
                responder
                    .send(match outcome {
                        Outcome::Set(name) => Ok(name),
                        Outcome::Unset => Ok(""),
                        Outcome::NoTable => Err(fnet_routes::StateGetRouteTableNameError::NoTable),
                    })
                    .unwrap_or_log("failed to respond");
            }
        }
    }

    fn handle_rule_op(
        ctx: &mut Ctx,
        rule_table: &mut RuleTable<I>,
        route_tables: &HashMap<TableId<I>, Table<I>>,
        op: RuleOp<I>,
        rules_update_dispatcher: &rules_state::RuleUpdateDispatcher<I>,
    ) -> Result<(), fnet_routes_admin::RuleSetError> {
        let updates =
            match op {
                RuleOp::RemoveSet { priority } => {
                    let removed = rule_table.remove_rule_set(priority);
                    itertools::Either::Left(removed.into_iter().map(watcher::Update::Removed))
                }
                RuleOp::Add { priority, index, matcher, action } => {
                    rule_table.add_rule(priority, index, matcher.clone(), action)?;
                    let matcher = matcher.into();
                    info!("Added PBR rule: {priority:?}, {index:?}, {matcher:?}, {action:?}");
                    itertools::Either::Right(std::iter::once(watcher::Update::Added(
                        InstalledRule { priority, index, matcher, action },
                    )))
                }
                RuleOp::Remove { priority, index } => {
                    let removed = rule_table.remove_rule(priority, index)?;
                    let InstalledRule { priority, index, matcher, action } = &removed;
                    info!("Removed PBR rule: {priority:?}, {index:?}, {matcher:?}, {action:?}");
                    itertools::Either::Right(std::iter::once(watcher::Update::Removed(removed)))
                }
            };
        Self::install_rules_and_notify_watchers(
            ctx,
            rule_table,
            route_tables,
            updates,
            rules_update_dispatcher,
        );
        Ok(())
    }

    fn install_rules_and_notify_watchers(
        ctx: &mut Ctx,
        rule_table: &mut RuleTable<I>,
        route_tables: &HashMap<TableId<I>, Table<I>>,
        updates: impl IntoIterator<Item = watcher::Update<RuleEvent<I>>>,
        rules_update_dispatcher: &rules_state::RuleUpdateDispatcher<I>,
    ) {
        let rules = rule_table
            .iter()
            .map(|r| {
                // Rule table should only contain validated rules.
                to_core_rule(ctx, r, route_tables).expect("the rule referenced an invalid table id")
            })
            .collect::<Vec<_>>();
        ctx.api().routes::<I>().set_rules(rules);
        for update in updates {
            rules_update_dispatcher.notify(update).expect("failed to notify an update")
        }
    }

    fn handle_route_table_authentication(
        route_tables: &HashMap<TableId<I>, Table<I>>,
        table_id: TableId<I>,
        token: zx::Event,
    ) -> Result<(), fnet_routes_admin::AuthenticateForRouteTableError> {
        let route_table = route_tables
            .get(&table_id)
            .ok_or(fnet_routes_admin::AuthenticateForRouteTableError::InvalidAuthentication)?;
        let stored_koid = route_table.token.basic_info().expect("failed to get basic info").koid;
        if token.basic_info().expect("failed to get basic info").koid != stored_koid {
            Err(fnet_routes_admin::AuthenticateForRouteTableError::InvalidAuthentication)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone)]
struct InvalidTableError;

#[netstack3_core::context_ip_bounds(I, BindingsCtx)]
fn to_core_rule<I: netstack3_core::IpExt>(
    ctx: &mut Ctx,
    rule: &rules_admin::Rule<I>,
    tables: &HashMap<TableId<I>, Table<I>>,
) -> Result<netstack3_core::routes::Rule<I, DeviceId, BindingsCtx>, InvalidTableError> {
    let rules_admin::Rule { matcher, action } = rule;
    let action = match action {
        RuleAction::Unreachable => netstack3_core::routes::RuleAction::Unreachable,
        RuleAction::Lookup(table_id) => {
            let table_id = TableId::new(table_id.get()).ok_or(InvalidTableError)?;
            let core_id = &tables.get(&table_id).ok_or(InvalidTableError)?.core_id;
            let id = match core_id {
                CoreId::Main => ctx.api().routes::<I>().main_table_id(),
                CoreId::User(id) => id.clone(),
            };
            netstack3_core::routes::RuleAction::Lookup(id)
        }
    };
    Ok(netstack3_core::routes::Rule { matcher: matcher.clone().into(), action })
}

#[netstack3_core::context_ip_bounds(I, BindingsCtx)]
fn to_entry<I: netstack3_core::IpExt>(
    ctx: &mut Ctx,
    addable_entry: netstack3_core::routes::AddableEntry<I::Addr, DeviceId>,
) -> netstack3_core::routes::Entry<I::Addr, DeviceId> {
    let device_metric = ctx.api().device_ip::<I>().get_routing_metric(&addable_entry.device);
    addable_entry.resolve_metric(device_metric)
}

#[netstack3_core::context_ip_bounds(I, BindingsCtx)]
fn handle_single_table_route_change<I>(
    table: &mut Table<I>,
    table_id: TableId<I>,
    ctx: &mut Ctx,
    change: Change<I>,
    route_update_dispatcher: &state::RouteUpdateDispatcher<I>,
    default_routes_by_device: &mut DefaultRoutesByDevice,
) -> Result<ChangeOutcome, ChangeError>
where
    I: IpExt + FidlRouteAdminIpExt,
{
    enum TableChange<I: Ip, Iter> {
        Add(netstack3_core::routes::Entry<I::Addr, DeviceId>),
        Remove(Iter),
    }

    let table_change: TableChange<I, _> = match change {
        Change::RouteOp(RouteOp::Add(addable_entry), set) => {
            let set = set.upgrade().ok_or(ChangeError::SetRemoved)?;
            let addable_entry = addable_entry
                .try_map_device_id(|d| d.upgrade().ok_or(ChangeError::DeviceRemoved))?;
            match table.insert(addable_entry, set) {
                TableModifyResult::NoChange => return Ok(ChangeOutcome::NoChange),
                TableModifyResult::SetChanged => return Ok(ChangeOutcome::Changed),
                TableModifyResult::TableChanged((addable_entry, _generation)) => {
                    TableChange::Add(to_entry::<I>(ctx, addable_entry))
                }
            }
        }
        Change::RouteOp(RouteOp::RemoveMatching { subnet, device, gateway, metric }, set) => {
            match table.remove(
                |entry| {
                    entry.subnet == subnet
                        && entry.device == device
                        && gateway.matches(&entry.gateway)
                        && metric.matches(&entry.metric)
                },
                set,
            ) {
                TableModifyResult::NoChange => return Ok(ChangeOutcome::NoChange),
                TableModifyResult::SetChanged => return Ok(ChangeOutcome::Changed),
                TableModifyResult::TableChanged(entries) => {
                    TableChange::Remove(itertools::Either::Left(entries.into_iter()))
                }
            }
        }
        Change::RemoveMatchingDevice(device) => {
            let result = table.remove(
                |entry| entry.device == device,
                // NB: we use `SetMembership::Global` here to remove routes on
                // this device from the table regardless of the sets they belong
                // to.
                SetMembership::Global,
            );
            match result {
                TableModifyResult::NoChange => return Ok(ChangeOutcome::NoChange),
                TableModifyResult::SetChanged => {
                    unreachable!(
                        "TableModifyResult::SetChanged cannot be returned \
                         when globally removing a route"
                    )
                }
                TableModifyResult::TableChanged(routes_from_table) => {
                    TableChange::Remove(itertools::Either::Right(itertools::Either::Left(
                        routes_from_table.into_iter(),
                    )))
                }
            }
        }
        Change::RemoveSet(set) => {
            let entries = table.remove_user_set(set);
            if entries.is_empty() {
                return Ok(ChangeOutcome::NoChange);
            }
            TableChange::Remove(itertools::Either::Right(itertools::Either::Right(
                itertools::Either::Left(entries.into_iter()),
            )))
        }
        Change::RemoveTable(_table_id) => {
            let removed = std::mem::take(&mut table.inner)
                .into_iter()
                .map(|(entry, EntryData { generation, set_membership: _ })| (entry, generation));
            TableChange::Remove(itertools::Either::Right(itertools::Either::Right(
                itertools::Either::Right(removed),
            )))
        }
    };

    let new_routes = table
        .inner
        .iter()
        .map(|(entry, data)| {
            let device_metric = ctx.api().device_ip::<I>().get_routing_metric(&entry.device);
            entry.clone().resolve_metric(device_metric).with_generation(data.generation)
        })
        .collect::<Vec<_>>();
    let core_id = match &table.core_id {
        CoreId::Main => {
            let main_table_id = ctx.api().routes::<I>().main_table_id();
            Cow::Owned(main_table_id)
        }
        CoreId::User(core_id) => Cow::Borrowed(core_id),
    };
    ctx.api().routes::<I>().set_routes(core_id.borrow(), new_routes);

    match table_change {
        TableChange::Add(entry) => {
            if entry.subnet.prefix() == 0 {
                if default_routes_by_device.add(entry.device.clone()) == 1 {
                    ctx.bindings_ctx().notify_interface_update(
                        &entry.device,
                        crate::bindings::InterfaceUpdate::DefaultRouteChanged {
                            version: I::VERSION,
                            has_default_route: true,
                        },
                    )
                }
            }
            let installed_route = EntryAndTableId { entry, table_id }
                .try_into_fidl_with_ctx(ctx.bindings_ctx())
                .expect("failed to convert route to FIDL");
            route_update_dispatcher
                .notify(watcher::Update::Added(installed_route))
                .expect("failed to notify route update dispatcher");
        }
        TableChange::Remove(removed) => {
            // Clone the Ctx so we can capture it in the mapping iterator. This
            // is cheaper than collecting into a Vec to eliminate the borrow.
            let mut ctx_clone = ctx.clone();
            let removed = removed.map(|(entry, _generation)| EntryAndTableId {
                entry: to_entry::<I>(&mut ctx_clone, entry),
                table_id,
            });
            notify_removed_routes::<I>(
                ctx.bindings_ctx(),
                route_update_dispatcher,
                removed,
                default_routes_by_device,
            );
        }
    };

    Ok(ChangeOutcome::Changed)
}

#[netstack3_core::context_ip_bounds(I, BindingsCtx)]
fn handle_route_change<I>(
    tables: &mut HashMap<TableId<I>, Table<I>>,
    ctx: &mut Ctx,
    change: Change<I>,
    route_update_dispatcher: &state::RouteUpdateDispatcher<I>,
    default_routes_by_device: &mut DefaultRoutesByDevice,
) -> Result<ChangeOutcome, ChangeError>
where
    I: IpExt + FidlRouteAdminIpExt,
{
    debug!("routes::handle_change {change:?}");

    fn one_table<I: Ip>(
        tables: &mut HashMap<TableId<I>, Table<I>>,
        table_id: TableId<I>,
    ) -> std::iter::Once<(TableId<I>, &'_ mut Table<I>)> {
        let table = tables.get_mut(&table_id).expect("missing table {table_id:?}");
        std::iter::once((table_id, table))
    }

    let mut tables = match &change {
        Change::RouteOp(_, SetMembership::User(weak_set)) | Change::RemoveSet(weak_set) => {
            itertools::Either::Left(one_table(
                tables,
                weak_set.upgrade().ok_or(ChangeError::SetRemoved)?.table(),
            ))
        }
        Change::RemoveMatchingDevice(_) => {
            itertools::Either::Right(tables.iter_mut().map(|(k, v)| (k.clone(), v)))
        }
        // The following routes set memberships refer to the main table.
        // TODO(https://fxbug.dev/339567592): GlobalRouteSet should be aware of
        // the route table as well.
        Change::RouteOp(_, SetMembership::Global) => {
            itertools::Either::Left(one_table(tables, main_table_id::<I>()))
        }
        // The change itself does not carry information about the destination
        // table, instead we deduce the table from the interface in the route:
        // use the interface-local route table if configured, otherwise use the
        // main table.
        Change::RouteOp(op, SetMembership::CoreNdp)
        | Change::RouteOp(op, SetMembership::InitialDeviceRoutes)
        | Change::RouteOp(op, SetMembership::Loopback) => {
            let local_table_id = op
                .get_interface_local_table()
                .map_err(|DeviceRemovedError| ChangeError::DeviceRemoved)?;
            itertools::Either::Left(one_table(
                tables,
                local_table_id.unwrap_or_else(main_table_id::<I>),
            ))
        }
        Change::RemoveTable(table_id) => itertools::Either::Left(one_table(tables, *table_id)),
    };

    tables.try_fold(ChangeOutcome::NoChange, |change_so_far, (table_id, table)| {
        let change_outcome = handle_single_table_route_change(
            table,
            table_id,
            ctx,
            change.clone(),
            route_update_dispatcher,
            default_routes_by_device,
        )?;
        Ok(match (change_so_far, change_outcome) {
            (ChangeOutcome::NoChange, ChangeOutcome::NoChange) => ChangeOutcome::NoChange,
            (ChangeOutcome::Changed, _) | (_, ChangeOutcome::Changed) => ChangeOutcome::Changed,
        })
    })
}

fn notify_removed_routes<I: Ip>(
    bindings_ctx: &crate::bindings::BindingsCtx,
    dispatcher: &state::RouteUpdateDispatcher<I>,
    removed_routes: impl IntoIterator<Item = EntryAndTableId<I>>,
    default_routes_by_device: &mut DefaultRoutesByDevice,
) {
    for EntryAndTableId { entry, table_id } in removed_routes {
        // Only flip the `has_default_route` bit in the interface watcher if
        // this is the main route table.
        if entry.subnet.prefix() == 0 {
            if default_routes_by_device.remove(entry.device.clone()) == 0 {
                bindings_ctx.notify_interface_update(
                    &entry.device,
                    crate::bindings::InterfaceUpdate::DefaultRouteChanged {
                        version: I::VERSION,
                        has_default_route: false,
                    },
                )
            }
        }
        let installed_route = EntryAndTableId { entry, table_id }
            .try_into_fidl_with_ctx(bindings_ctx)
            .expect("failed to convert route to FIDL");
        dispatcher
            .notify(watcher::Update::Removed(installed_route))
            .expect("failed to notify route update dispatcher");
    }
}

#[derive(Clone)]
pub(crate) struct ChangeSink {
    v4: Changes<Ipv4>,
    v6: Changes<Ipv6>,
}

#[must_use = "route changes won't be applied without running the ChangeRunner"]
pub(crate) struct ChangeRunner {
    v4: State<Ipv4>,
    v6: State<Ipv6>,
}

#[derive(Default, Clone)]
pub(crate) struct Dispatchers<I: Ip> {
    pub(crate) route_update_dispatcher: state::RouteUpdateDispatcher<I>,
    pub(crate) rule_update_dispatcher: rules_state::RuleUpdateDispatcher<I>,
}

impl ChangeRunner {
    pub(crate) fn update_dispatchers(&self) -> (Dispatchers<Ipv4>, Dispatchers<Ipv6>) {
        let Self { v4, v6 } = self;
        (v4.dispatchers.clone(), v6.dispatchers.clone())
    }

    pub(crate) async fn run(&mut self, ctx: Ctx) {
        let Self { v4, v6 } = self;
        let v4_fut = v4.run_changes(ctx.clone());
        let v6_fut = v6.run_changes(ctx);
        let ((), ()) = futures::future::join(v4_fut, v6_fut).await;
    }
}

pub(crate) fn create_sink_and_runner() -> (ChangeSink, ChangeRunner) {
    fn create<I: FidlRouteAdminIpExt>() -> (Changes<I>, State<I>) {
        let (table_work_sink, table_work_receiver) = mpsc::unbounded();
        let mut tables = HashMap::new();
        let main_table_id = main_table_id::<I>();

        let main_table = Table::new(
            netstack3_core::routes::Generation::initial(),
            CoreId::Main,
            Some(format!(
                "main_{}",
                match I::VERSION {
                    IpVersion::V4 => "v4",
                    IpVersion::V6 => "v6",
                }
            )),
        );
        let main_table_token = main_table.token.clone();
        assert_matches!(tables.insert(main_table_id, main_table), None);

        let (main_table_route_work_sink, main_table_route_work_receiver) = mpsc::unbounded();
        let route_work_receivers =
            RouteWorkReceivers::new(main_table_route_work_receiver.into_future());
        let (new_rule_set_sink, new_rule_set_receiver) = mpsc::unbounded();

        let state = State {
            table_work_receiver,
            tables,
            rules: Default::default(),
            route_work_receivers,
            new_rule_set_receiver,
            last_table_id: main_table_id,
            dispatchers: Default::default(),
            default_routes_by_device: Default::default(),
        };
        (
            Changes {
                table_work_sink,
                main_table_route_work_sink,
                main_table_token,
                new_rule_set_sink,
            },
            state,
        )
    }
    let (v4, v4_state) = create::<Ipv4>();
    let (v6, v6_state) = create::<Ipv6>();
    (ChangeSink { v4, v6 }, ChangeRunner { v4: v4_state, v6: v6_state })
}

impl ChangeSink {
    /// Closes the channels over which routes change requests are sent, causing
    /// [`ChangeRunner::run`] to exit.
    pub(crate) fn close_senders(&self) {
        let Self { v4, v6 } = self;
        v4.close_senders();
        v6.close_senders();
    }

    pub(crate) fn fire_main_table_route_change_and_forget<I: Ip>(&self, change: Change<I>) {
        let sender = self.main_table_route_work_sink::<I>();
        let item = RouteWorkItem { change, responder: None };
        match sender.unbounded_send(item) {
            Ok(()) => (),
            Err(e) => warn!(
                "failed to send route change {:?} because route change sink is closed",
                e.into_inner()
            ),
        };
    }

    pub(crate) fn send_main_table_route_change<I: Ip>(
        &self,
        change: Change<I>,
    ) -> impl Future<Output = Result<ChangeOutcome, ChangeError>> {
        let sender = self.main_table_route_work_sink::<I>();
        let (responder, receiver) = oneshot::channel();
        let item = RouteWorkItem { change, responder: Some(responder) };
        match sender.unbounded_send(item) {
            Ok(()) => receiver.map(|r| r.expect("responder should not be dropped")).left_future(),
            Err(e) => {
                let _: mpsc::TrySendError<_> = e;
                futures::future::ready(Err(ChangeError::TableRemoved)).right_future()
            }
        }
    }

    pub(crate) async fn add_table<I: Ip>(
        &self,
        name: Option<String>,
    ) -> Result<
        (TableId<I>, mpsc::UnboundedSender<RouteWorkItem<I>>, Arc<zx::Event>),
        TableIdOverflowsError,
    > {
        let sender = &self.changes::<I>().table_work_sink;
        let (responder, receiver) = oneshot::channel();
        let item = TableWorkItem { op: TableOp::AddTable(AddTableOp { name, responder }) };
        sender.unbounded_send(item).expect("change runner shut down before services are shut down");
        receiver.map(|r| r.expect("responder should not be dropped")).await.map(
            |AddTableOutcome { table_id, route_work_sink, token }| {
                (table_id, route_work_sink, token)
            },
        )
    }

    async fn new_rule_set<I: Ip>(
        &self,
        priority: RuleSetPriority,
        rule_set_work_receiver: mpsc::UnboundedReceiver<RuleWorkItem<I>>,
    ) -> Result<(), SetPriorityConflict> {
        let sender = &self.changes::<I>().new_rule_set_sink;
        let (responder, receiver) = oneshot::channel();
        sender
            .unbounded_send(NewRuleSet { priority, rule_set_work_receiver, responder })
            .expect("change runner shut down before the services are shut down");
        receiver.await.expect("responder should not be dropped")
    }

    pub(crate) fn main_table_route_work_sink<I: Ip>(
        &self,
    ) -> &mpsc::UnboundedSender<RouteWorkItem<I>> {
        &self.changes::<I>().main_table_route_work_sink
    }

    pub(crate) fn main_table_token<I: Ip>(&self) -> &Arc<zx::Event> {
        &self.changes::<I>().main_table_token
    }

    pub(crate) async fn add_default_rule<I: Ip>(&self) {
        let (rule_work_sink, receiver) = mpsc::unbounded();
        self.new_rule_set::<I>(DEFAULT_RULE_SET_PRIORITY, receiver)
            .await
            .expect("failed to add a new rule set");
        let (responder, receiver) = oneshot::channel();
        rule_work_sink
            .unbounded_send(RuleWorkItem::RuleOp {
                op: RuleOp::Add {
                    priority: DEFAULT_RULE_SET_PRIORITY,
                    index: RuleIndex::new(0),
                    matcher: Default::default(),
                    action: RuleAction::Lookup(main_table_id::<I>().into()),
                },
                responder,
            })
            .expect("failed to send work item");
        receiver.await.expect("responder dropped").expect("failed to add the default rule");
    }

    pub(crate) fn get_route_table_name<I: Ip>(
        &self,
        table_id: TableId<I>,
        responder: fnet_routes::StateGetRouteTableNameResponder,
    ) {
        self.changes::<I>()
            .table_work_sink
            .unbounded_send(TableWorkItem {
                op: TableOp::GetRouteTableName(GetRouteTableNameOp { table_id, responder }),
            })
            .expect("failed to send work item");
    }

    fn changes<I: Ip>(&self) -> &Changes<I> {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct ChangesOutput<'a, I: Ip> {
            changes: &'a Changes<I>,
        }

        let ChangesOutput { changes } = I::map_ip_out(
            self,
            |ChangeSink { v4, v6: _ }| ChangesOutput { changes: &v4 },
            |ChangeSink { v4: _, v6 }| ChangesOutput { changes: &v6 },
        );
        changes
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use futures::channel::oneshot;
    use ip_test_macro::ip_test;
    use net_declare::{net_subnet_v4, net_subnet_v6};
    use netstack3_core::routes::{AddableEntry, AddableMetric, Entry, Metric, RawMetric};

    use super::*;
    use crate::bindings::integration_tests::{
        StackSetupBuilder, TestSetup, TestSetupBuilder, set_logger_for_test,
    };

    #[netstack3_core::context_ip_bounds(I, BindingsCtx)]
    #[fixture::teardown(TestSetup::shutdown)]
    #[ip_test(I)]
    #[fuchsia::test(logging = false)]
    async fn table_added_in_both_core_and_bindings<I: IpExt>() {
        set_logger_for_test();
        let t = TestSetupBuilder::new()
            .add_endpoint()
            .add_stack(StackSetupBuilder::new().add_endpoint(1, None))
            .build()
            .await;

        let test_stack = t.get(0);
        let if_id = test_stack.get_endpoint_id(1);

        let mut ctx = test_stack.ctx();
        let (table_id, work_sink, _token) = ctx
            .bindings_ctx()
            .routes
            .add_table::<I>(None)
            .await
            .expect("failed to add a new table");
        let subnet = I::map_ip_out(
            (),
            |()| net_subnet_v4!("192.168.0.0/24"),
            |()| net_subnet_v6!("2001:db8::/64"),
        );
        let route_set = netstack3_core::sync::PrimaryRc::new(admin::UserRouteSetId::new(table_id));
        let weak_route_set = netstack3_core::sync::PrimaryRc::downgrade(&route_set);
        let device =
            ctx.bindings_ctx().devices.get_core_id(if_id).expect("failed to get device Id");
        let weak_device = device.downgrade();
        let (responder, receiver) = oneshot::channel();
        work_sink
            .unbounded_send(RouteWorkItem {
                change: Change::RouteOp(
                    RouteOp::Add(AddableEntry {
                        subnet,
                        device: weak_device,
                        gateway: None,
                        metric: AddableMetric::ExplicitMetric(RawMetric(0)),
                        route_preference: Default::default(),
                    }),
                    SetMembership::User(weak_route_set),
                ),
                responder: Some(responder),
            })
            .expect("failed to send route work");
        assert_matches!(receiver.await.expect("failed to add route"), Ok(ChangeOutcome::Changed));
        let main_table_id = ctx.api().routes().main_table_id();
        let tables = ctx.api().routes().list_table_ids();
        assert_eq!(tables.len(), 2);
        let core_id = tables
            .into_iter()
            .filter(|id| id != &main_table_id)
            .next()
            .expect("failed to retrieve the table ID");

        let mut routes = Vec::<Entry<_, _>>::new();
        ctx.api().routes().collect_routes_into(&core_id, &mut routes);

        assert_eq!(
            routes,
            &[Entry {
                subnet,
                device,
                gateway: None,
                metric: Metric::ExplicitMetric(RawMetric(0)),
                route_preference: Default::default(),
            }]
        );

        // Drop the strong reference first so that we can remove the primary reference later.
        std::mem::drop(core_id);

        let (responder, receiver) = oneshot::channel();
        work_sink
            .unbounded_send(RouteWorkItem {
                change: Change::RemoveTable(table_id),
                responder: Some(responder),
            })
            .expect("failed to send route work");
        assert_matches!(receiver.await.expect("failed to add route"), Ok(ChangeOutcome::Changed));

        let tables = ctx.api().routes().list_table_ids();
        // We don't implement Debug on IDs.
        assert!(tables == &[main_table_id]);

        t
    }
}
