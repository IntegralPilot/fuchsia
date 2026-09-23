// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU32;

use fidl::endpoints::ProtocolMarker;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fidl_fuchsia_net_interfaces_ext::admin::TerminalError;
use fidl_fuchsia_net_routes_admin::GetInterfaceLocalTableError;
use fidl_fuchsia_net_routes_ext as fnet_routes_ext;

use assert_matches::assert_matches;
use net_types::ip::{GenericOverIp, Ip};

use crate::routes::{MAIN_ROUTE_TABLE_INDEX, NetlinkRouteMessage};

/// The index of a route table (in netlink's view of indices).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub(crate) struct NetlinkRouteTableIndex(u32);

impl NetlinkRouteTableIndex {
    pub(crate) const fn new(index: u32) -> Self {
        Self(index)
    }

    pub(crate) const fn get(self) -> u32 {
        let Self(index) = self;
        index
    }
}

/// The index of a route table (in netlink's view of indices).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub(crate) struct NonZeroNetlinkRouteTableIndex(NonZeroU32);

impl NonZeroNetlinkRouteTableIndex {
    pub(crate) const fn new(index: NetlinkRouteTableIndex) -> Option<Self> {
        let NetlinkRouteTableIndex(index) = index;
        // NB: `Option::map` is not available in `const`
        match NonZeroU32::new(index) {
            None => None,
            Some(index) => Some(Self(index)),
        }
    }

    pub(crate) const fn new_non_zero(index: NonZeroU32) -> Self {
        Self(index)
    }

    pub(crate) const fn get(self) -> NonZeroU32 {
        let Self(index) = self;
        index
    }
}

impl From<NonZeroNetlinkRouteTableIndex> for NetlinkRouteTableIndex {
    fn from(index: NonZeroNetlinkRouteTableIndex) -> Self {
        Self(index.get().get())
    }
}

/// Indicates that a given table should be considered for cleanup.
/// It should only be deleted if there are no longer any rules or routes referencing it.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) struct TableNeedsCleanup(
    pub(crate) fnet_routes_ext::TableId,
    pub(crate) NetlinkRouteTableIndex,
);

#[derive(derivative::Derivative)]
#[derivative(Debug(bound = ""))]
pub(crate) enum RouteTable<
    I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
> {
    Managed(ManagedRouteTable<I>),
    Unmanaged(UnmanagedTable<I>),
}

impl<I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt>
    RouteTable<I>
{
    pub(crate) fn fidl_table_id(&self) -> fnet_routes_ext::TableId {
        match self {
            RouteTable::Managed(route_table) => route_table.fidl_table_id,
            RouteTable::Unmanaged(route_table) => route_table.fidl_table_id,
        }
    }
}

/// State tracked for a route table managed by the netlink worker.
#[derive(derivative::Derivative)]
#[derivative(Debug(bound = ""))]
pub(crate) struct ManagedRouteTable<
    I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
> {
    /// The route table FIDL proxy backing this route table.
    #[derivative(Debug = "ignore")]
    pub(crate) route_table_proxy: <I::RouteTableMarker as ProtocolMarker>::Proxy,

    /// A route set derived from the corresponding FIDL route table.
    #[derivative(Debug = "ignore")]
    pub(crate) route_set_proxy: <I::RouteSetMarker as ProtocolMarker>::Proxy,

    /// The ID of the corresponding FIDL route table.
    pub(crate) fidl_table_id: fnet_routes_ext::TableId,

    /// Whether the netlink worker's fuchsia.net.routes.admin.RuleSet has been authenticated
    /// to install rules referencing this table.
    pub(crate) rule_set_authenticated: bool,
}

/// A reference to the main route table.
#[derive(derivative::Derivative)]
#[derivative(Debug(bound = ""))]
pub(crate) struct UnmanagedTable<
    I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
> {
    /// The route table FIDL proxy for the main route table.
    #[derivative(Debug = "ignore")]
    pub(crate) route_table_proxy: <I::RouteTableMarker as ProtocolMarker>::Proxy,

    /// A route set derived from the main FIDL route table.
    #[derivative(Debug = "ignore")]
    pub(crate) route_set_proxy: <I::RouteSetMarker as ProtocolMarker>::Proxy,

    /// The ID of the main FIDL route table.
    pub(crate) fidl_table_id: fnet_routes_ext::TableId,

    /// Whether the netlink worker's fuchsia.net.routes.admin.RuleSet has been authenticated
    /// to install rules referencing this table.
    pub(crate) rule_set_authenticated: bool,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum NoInterfaceLocalTableError {
    #[error("the interface has been removed: {0:?}")]
    InterfaceRemoved(TerminalError<fidl_fuchsia_net_interfaces_admin::InterfaceRemovedReason>),
    #[error("the interface does not have a local table")]
    NoInterfaceLocaltable,
}

impl<I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt>
    UnmanagedTable<I>
{
    pub(crate) async fn interface_local(
        control: &fnet_interfaces_ext::admin::Control,
        route_table_provider: &<I::RouteTableProviderMarker as ProtocolMarker>::Proxy,
    ) -> Result<Self, NoInterfaceLocalTableError> {
        let grant = control
            .get_authorization_for_interface()
            .await
            .map_err(NoInterfaceLocalTableError::InterfaceRemoved)?;
        let proof = fnet_interfaces_ext::admin::proof_from_grant(&grant);
        let local_table =
            fnet_routes_ext::admin::get_interface_local_table::<I>(route_table_provider, proof)
                .await
                .expect("fidl failure getting the local table")
                .map_err(|err| match err {
                    GetInterfaceLocalTableError::NoLocalRouteTable => {
                        NoInterfaceLocalTableError::NoInterfaceLocaltable
                    }
                    GetInterfaceLocalTableError::InvalidAuthentication
                    | GetInterfaceLocalTableError::__SourceBreaking { .. } => {
                        panic!("unexpected error getting the local table: {err:?}")
                    }
                })?;
        // From this point, we know that the local table exists and we cannot
        // recover from the errors below.
        let route_set_proxy = fnet_routes_ext::admin::new_route_set::<I>(&local_table)
            .expect("failed to create the route set");
        let fidl_table_id = fnet_routes_ext::admin::get_table_id::<I>(&local_table)
            .await
            .expect("failed to get the table ID");
        Ok(Self {
            route_table_proxy: local_table,
            route_set_proxy,
            fidl_table_id,
            rule_set_authenticated: false,
        })
    }
}

/// Returned as the `Err` variant when the user attempts to remove the main table.
#[derive(Debug, Copy, Clone)]
pub(crate) struct AttemptToRemoveMainTableError;

#[derive(GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub(crate) struct RouteTableMap<
    I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
> {
    route_tables: HashMap<NetlinkRouteTableIndex, RouteTable<I>>,
    fidl_table_ids: HashMap<fnet_routes_ext::TableId, NetlinkRouteTableIndex>,
    route_table_provider: <I::RouteTableProviderMarker as ProtocolMarker>::Proxy,
}

impl<I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt>
    RouteTableMap<I>
{
    pub(crate) fn new(
        main_route_table_proxy: <I::RouteTableMarker as ProtocolMarker>::Proxy,
        main_route_table_id: fnet_routes_ext::TableId,
        unmanaged_route_set_proxy: <I::RouteSetMarker as ProtocolMarker>::Proxy,
        route_table_provider: <I::RouteTableProviderMarker as ProtocolMarker>::Proxy,
    ) -> Self {
        Self {
            route_tables: HashMap::from_iter(std::iter::once((
                MAIN_ROUTE_TABLE_INDEX,
                RouteTable::Unmanaged(UnmanagedTable {
                    route_table_proxy: main_route_table_proxy,
                    fidl_table_id: main_route_table_id,
                    route_set_proxy: unmanaged_route_set_proxy,
                    rule_set_authenticated: false,
                }),
            ))),
            fidl_table_ids: HashMap::from_iter(std::iter::once((
                main_route_table_id,
                MAIN_ROUTE_TABLE_INDEX,
            ))),
            route_table_provider,
        }
    }

    /// Looks up the [`NetlinkRouteTableIndex`] for a FIDL table ID.
    pub(crate) fn get_netlink_id(
        &self,
        key: &fnet_routes_ext::TableId,
    ) -> Option<NetlinkRouteTableIndex> {
        self.fidl_table_ids.get(key).copied()
    }

    /// Gets a reference to the [`RouteTable`] for the given `key`.
    pub(crate) fn get(&self, key: &NetlinkRouteTableIndex) -> Option<&RouteTable<I>> {
        self.route_tables.get(key)
    }

    /// Gets a mutable reference to the [`RouteTable`] for the given `key`.
    pub(crate) fn get_mut(&mut self, key: &NetlinkRouteTableIndex) -> Option<&mut RouteTable<I>> {
        self.route_tables.get_mut(key)
    }

    /// Inserts a mapping from a [`NetlinkRouteTableIndex`] to a [`RouteTable`].
    pub(crate) fn insert(&mut self, key: NetlinkRouteTableIndex, value: RouteTable<I>) {
        let fidl_table_id = value.fidl_table_id();
        let prev = self.route_tables.insert(key, value);
        assert_matches!(prev, None, "{prev:?}");

        let prev = self.fidl_table_ids.insert(fidl_table_id, key);
        assert_matches!(
            prev,
            None,
            "fidl_table_id {fidl_table_id:?} already maps to netlink table {prev:?}"
        );
    }

    /// Removes a netlink-managed route table.
    pub(crate) fn remove(&mut self, key: NetlinkRouteTableIndex) -> Option<RouteTable<I>> {
        let table_id = self.route_tables.get(&key)?.fidl_table_id();
        Some(self.remove_table_by_fidl_id(table_id)?.expect("cannot remove the main table"))
    }

    /// Removes the [`RouteTable`] with the given FIDL table ID.
    pub(crate) fn remove_table_by_fidl_id(
        &mut self,
        table_id: fnet_routes_ext::TableId,
    ) -> Option<Result<RouteTable<I>, AttemptToRemoveMainTableError>> {
        if self
            .get_netlink_id(&table_id)
            .is_some_and(|netlink_id| netlink_id == MAIN_ROUTE_TABLE_INDEX)
        {
            return Some(Err(AttemptToRemoveMainTableError));
        }

        let netlink_index = self.fidl_table_ids.remove(&table_id)?;
        let removed = self.route_tables.remove(&netlink_index)?;
        assert_eq!(removed.fidl_table_id(), table_id);
        Some(Ok(removed))
    }

    pub(crate) fn route_table_provider(
        &self,
    ) -> &<I::RouteTableProviderMarker as ProtocolMarker>::Proxy {
        &self.route_table_provider
    }

    /// If a table corresponding to `key` is not already being tracked,
    /// creates a new [`ManagedRouteTable`] entry for this key.
    pub(crate) async fn create_managed_route_table_if_not_present(
        &mut self,
        key: NetlinkRouteTableIndex,
    ) {
        if self.route_tables.get(&key).is_some() {
            return;
        }

        // There's nothing graceful we can do if any of these operations fail, so we might as well
        // panic.
        let route_table_proxy = fnet_routes_ext::admin::new_route_table::<I>(
            &self.route_table_provider,
            Some(format!("netlink:{}", key.get())),
        )
        .expect("error creating new route table");
        let fidl_table_id = fnet_routes_ext::admin::get_table_id::<I>(&route_table_proxy)
            .await
            .expect("error getting table ID");
        let route_set_proxy = fnet_routes_ext::admin::new_route_set::<I>(&route_table_proxy)
            .expect("error creating new route set");
        self.insert(
            key,
            RouteTable::Managed(ManagedRouteTable {
                route_table_proxy,
                route_set_proxy,
                fidl_table_id,
                rule_set_authenticated: false,
            }),
        );
    }
}

/// All of the routes known to the netlink worker, and the tables in which they are installed.
/// This is populated via the fuchsia.net.routes watcher protocol, and may include routes in tables
/// that are not known to netlink (i.e. route tables that were created by other Fuchsia components
/// via fuchsia.net.routes.admin.RouteTableProvider).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FidlRouteMap<
    I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt,
> {
    routes: BTreeMap<
        fnet_routes_ext::Route<I>,
        BTreeMap<fnet_routes_ext::TableId, fnet_routes_ext::EffectiveRouteProperties>,
    >,
    by_tables: BTreeMap<
        fnet_routes_ext::TableId,
        BTreeMap<fnet_routes_ext::Route<I>, fnet_routes_ext::EffectiveRouteProperties>,
    >,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) enum RouteRemoveResult {
    DidNotExist,
    RemovedButTableNotEmpty(fnet_routes_ext::EffectiveRouteProperties),
    RemovedAndTableNewlyEmpty(fnet_routes_ext::EffectiveRouteProperties),
}

impl<I: fnet_routes_ext::FidlRouteIpExt + fnet_routes_ext::admin::FidlRouteAdminIpExt>
    FidlRouteMap<I>
{
    pub(crate) fn table_is_present(&self, table: fnet_routes_ext::TableId) -> bool {
        self.by_tables.contains_key(&table)
    }

    /// Returns whether the given route is installed in all of the specified FIDL tables.
    pub(crate) fn route_is_installed_in_tables<'a>(
        &'a self,
        route: &'a fnet_routes_ext::Route<I>,
        tables: impl IntoIterator<Item = &'a fnet_routes_ext::TableId>,
    ) -> bool {
        match self.routes.get(route) {
            None => false,
            Some(installed_tables) => {
                tables.into_iter().all(|table| installed_tables.contains_key(table))
            }
        }
    }

    /// Returns whether the given route is uninstalled from all of the specified FIDL tables.
    pub(crate) fn route_is_uninstalled_in_tables<'a>(
        &'a self,
        route: &'a fnet_routes_ext::Route<I>,
        tables: impl IntoIterator<Item = &'a fnet_routes_ext::TableId>,
    ) -> bool {
        match self.routes.get(route) {
            None => true,
            Some(installed_tables) => {
                tables.into_iter().all(|table| !installed_tables.contains_key(table))
            }
        }
    }

    /// Adds the route, table, and properties to the set of route-table pairs tracked by this map.
    pub(crate) fn add(
        &mut self,
        route: fnet_routes_ext::Route<I>,
        table: fnet_routes_ext::TableId,
        properties: fnet_routes_ext::EffectiveRouteProperties,
    ) -> Option<fnet_routes_ext::EffectiveRouteProperties> {
        let props_by_routes = self.routes.entry(route).or_default().insert(table, properties);
        let props_by_tables = self.by_tables.entry(table).or_default().insert(route, properties);
        assert_eq!(props_by_routes, props_by_tables);
        props_by_routes
    }

    /// Removes the (route, table) combination from this map.
    ///
    /// The returned [`RouteRemoveResult`] indicates the outcome of the removal and whether it
    /// resulted in a table becoming empty.
    pub(crate) fn remove(
        &mut self,
        route: fnet_routes_ext::Route<I>,
        table: fnet_routes_ext::TableId,
    ) -> RouteRemoveResult {
        let props_by_routes = match self.routes.entry(route) {
            Entry::Vacant(_) => None,
            Entry::Occupied(mut occupied_entry) => {
                let removed = occupied_entry.get_mut().remove(&table);
                if removed.is_some() && occupied_entry.get().is_empty() {
                    let _: BTreeMap<_, _> = occupied_entry.remove();
                }
                removed
            }
        };
        let props_by_tables = match self.by_tables.entry(table) {
            Entry::Vacant(_) => None,
            Entry::Occupied(mut occupied_entry) => {
                let removed_props = occupied_entry.get_mut().remove(&route);
                match removed_props {
                    None => None,
                    Some(removed_props) => {
                        let removed_last_route_from_table = if occupied_entry.get().is_empty() {
                            let _: BTreeMap<
                                fnet_routes_ext::Route<I>,
                                fnet_routes_ext::EffectiveRouteProperties,
                            > = occupied_entry.remove();
                            true
                        } else {
                            false
                        };
                        Some((removed_props, removed_last_route_from_table))
                    }
                }
            }
        };
        assert_eq!(props_by_routes, props_by_tables.map(|(props, _removed_last)| props));

        match props_by_tables {
            None => RouteRemoveResult::DidNotExist,
            Some((props, false)) => RouteRemoveResult::RemovedButTableNotEmpty(props),
            Some((props, true)) => RouteRemoveResult::RemovedAndTableNewlyEmpty(props),
        }
    }

    pub(crate) fn iter<'a>(
        &'a self,
    ) -> impl Iterator<
        Item = (
            &'a fnet_routes_ext::Route<I>,
            &'a BTreeMap<fnet_routes_ext::TableId, fnet_routes_ext::EffectiveRouteProperties>,
        ),
    > {
        self.routes.iter()
    }

    /// Returns an iterator over routes installed within the given table.
    pub(crate) fn iter_messages<'a>(
        &'a self,
        route_table_map: &'a RouteTableMap<I>,
        table_id: NetlinkRouteTableIndex,
    ) -> impl Iterator<Item = NetlinkRouteMessage> + 'a {
        route_table_map
            .get(&table_id)
            .and_then(|table| {
                let fidl_table_id = table.fidl_table_id();
                self.by_tables.get(&fidl_table_id).map(|routes| {
                    routes
                        .iter()
                        .map(move |(route, props)| fnet_routes_ext::InstalledRoute {
                            route: *route,
                            table_id: fidl_table_id,
                            effective_properties: *props,
                        })
                        .filter_map(move |installed_route| {
                            NetlinkRouteMessage::optionally_from(installed_route, table_id)
                        })
                })
            })
            .into_iter()
            .flatten()
    }

    pub(crate) fn iter_table<'a>(
        &'a self,
        table: fnet_routes_ext::TableId,
    ) -> impl Iterator<
        Item = (&'a fnet_routes_ext::Route<I>, &'a fnet_routes_ext::EffectiveRouteProperties),
    > {
        self.by_tables.get(&table).into_iter().flatten()
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use fidl_fuchsia_net_routes as fnet_routes;
    use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
    use fidl_fuchsia_net_routes_ext::admin::FidlRouteAdminIpExt;
    use itertools::Itertools as _;
    use net_types::ip::Ipv6;
    use proptest::arbitrary::Arbitrary;
    use proptest::prelude::*;
    use proptest::test_runner::Config;
    use proptest_support::failed_seeds;

    use super::*;

    impl Arbitrary for NetlinkRouteTableIndex {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with((): ()) -> Self::Strategy {
            // NB: use u8 to constrain the space enough to get some collisions.
            let unwanted = u8::try_from(crate::routes::MAIN_ROUTE_TABLE_INDEX.get())
                .expect("must be valid u8");

            <u8 as Arbitrary>::arbitrary_with(())
                .prop_filter("not including the main table", move |index| *index != unwanted)
                .prop_map(|index| NetlinkRouteTableIndex(index.into()))
                .boxed()
        }
    }

    #[derive(proptest_derive::Arbitrary, Debug, PartialEq, Eq, Hash, Copy, Clone)]
    struct FidlTableId(u8);

    impl From<FidlTableId> for fnet_routes_ext::TableId {
        fn from(id: FidlTableId) -> Self {
            Self::new(id.0.into())
        }
    }

    const NEVER_GENERATED_TABLE_ID: fnet_routes_ext::TableId = fnet_routes_ext::TableId::new(
        // NB: we know we never generate this because 256 is out of bounds for a u8.
        256,
    );

    #[derive(proptest_derive::Arbitrary, Debug, PartialEq, Eq, Hash)]
    enum RouteTableMapOp {
        Insert(NetlinkRouteTableIndex),
        Remove(NetlinkRouteTableIndex),
    }

    fn collect_and_assert_all_unique(
        iter: impl Iterator<Item = fnet_routes_ext::TableId>,
    ) -> HashSet<fnet_routes_ext::TableId> {
        let vec = iter.collect::<Vec<_>>();
        let count_from_vec = vec.len();
        let set = vec.into_iter().collect::<HashSet<_>>();
        assert_eq!(set.len(), count_from_vec);
        set
    }

    #[derive(Copy, Clone, Debug)]
    struct TestRoute(fnet_routes_ext::Route<Ipv6>);

    // The contents don't really matter as the routes just need to be distinct according
    // to even 1 field (in this case the interface).
    const fn build_test_route(outbound_interface: u64) -> TestRoute {
        TestRoute(fnet_routes_ext::Route {
            destination: net_declare::net_subnet_v6!("2001:db8:1::/64"),
            action: fnet_routes_ext::RouteAction::Forward(fnet_routes_ext::RouteTarget {
                outbound_interface,
                next_hop: None,
            }),
            properties: fnet_routes_ext::RouteProperties {
                specified_properties: fnet_routes_ext::SpecifiedRouteProperties {
                    metric: fnet_routes::SpecifiedMetric::ExplicitMetric(1),
                },
            },
        })
    }

    const TEST_ROUTES: [TestRoute; 5] = [
        build_test_route(1),
        build_test_route(2),
        build_test_route(3),
        build_test_route(4),
        build_test_route(5),
    ];

    impl Arbitrary for TestRoute {
        type Parameters = ();
        type Strategy = proptest::sample::Select<Self>;

        fn arbitrary_with((): ()) -> Self::Strategy {
            proptest::sample::select(&TEST_ROUTES)
        }
    }

    #[derive(proptest_derive::Arbitrary, Copy, Clone, Debug)]
    struct TestEffectiveRouteProperties(u32);

    impl From<TestEffectiveRouteProperties> for fnet_routes_ext::EffectiveRouteProperties {
        fn from(TestEffectiveRouteProperties(metric): TestEffectiveRouteProperties) -> Self {
            fnet_routes_ext::EffectiveRouteProperties { metric }
        }
    }

    #[derive(proptest_derive::Arbitrary, Copy, Clone, Debug)]
    enum FidlRouteMapOp {
        Add(TestRoute, FidlTableId, TestEffectiveRouteProperties),
        Remove(TestRoute, FidlTableId),
    }

    proptest! {
        #![proptest_config(Config {
            // Add all failed seeds here.
            failure_persistence: failed_seeds!(),
            ..Config::default()
        })]

        // Test that the route table map preserves a 1:1 mapping between
        // `NetlinkRouteTableIndex` and `fnet_routes_ext::TableId` after arbitrary
        // operations.
        #[test]
        fn route_table_map_is_bijection(ops_to_try: Vec<RouteTableMapOp>) {
            // We create a bunch of proxies that go unused in this test. In order for this to
            // succeed we must have an executor.
            let _executor = fuchsia_async::TestExecutor::new();

            let (main_route_table_proxy, _server_end) =
                fidl::endpoints::create_proxy::<<Ipv6 as FidlRouteAdminIpExt>::RouteTableMarker>();
            let (unmanaged_route_set_proxy, _unmanaged_route_set_server_end) =
                fidl::endpoints::create_proxy::<<Ipv6 as FidlRouteAdminIpExt>::RouteSetMarker>();
            let (route_table_provider, _server_end) = fidl::endpoints::create_proxy::<
                <Ipv6 as FidlRouteAdminIpExt>::RouteTableProviderMarker,
            >();

            let mut route_table_map = RouteTableMap::<Ipv6>::new(
                main_route_table_proxy,
                fnet_routes_ext::TableId::new(
                    // arbitrary
                    6,
                ),
                unmanaged_route_set_proxy,
                route_table_provider,
            );

            let choose_unused_fidl_id = move |map: &RouteTableMap<Ipv6>| {
                // Assert that the existing set of FIDL ids tracked in the table is consistent.
                let used_fidl_ids_according_to_fidl_table_ids = collect_and_assert_all_unique(
                    map.fidl_table_ids
                        .keys()
                        .copied()
                );

                let used_fidl_ids_according_to_route_tables = collect_and_assert_all_unique(
                    map.route_tables
                        .values()
                        .map(|table| table.fidl_table_id())
                );

                assert_eq!(
                    used_fidl_ids_according_to_fidl_table_ids,
                    used_fidl_ids_according_to_route_tables
                );

                // Then just use the lowest hitherto unused one.
                (0u32..)
                    .map(fnet_routes_ext::TableId::new)
                    .find(|id| !used_fidl_ids_according_to_route_tables.contains(id))
                    .expect("should not run out of IDs")
            };

            let test_remove =
                |map: &mut RouteTableMap<Ipv6>, netlink_id: NetlinkRouteTableIndex| {
                    // Before removing, check for consistency between presence according to
                    // `get_fidl_table_id` and according to `remove`.
                    let prev_fidl_table_id = map.get(&netlink_id).map(|table| table.fidl_table_id());

                    if let Some(table) = map.remove(netlink_id) {
                        assert_eq!(Some(table.fidl_table_id()), prev_fidl_table_id);
                    } else {
                        assert_eq!(prev_fidl_table_id, None);
                    }

                    // Now everything should indicate absence.
                    assert_matches!(map.get(&netlink_id), None);
                    assert_matches!(map.remove(netlink_id), None);
                    if let Some(prev_fidl_table_id) = prev_fidl_table_id {
                        assert_eq!(
                            map.get_netlink_id(&prev_fidl_table_id),
                            None
                        );
                    }
                };

            let test_insert =
                |map: &mut RouteTableMap<Ipv6>, netlink_id: NetlinkRouteTableIndex| {
                    // Callers are expected to avoid inserting duplicate netlink IDs.
                    // To ensure this, test removal as well first.
                    test_remove(map, netlink_id);

                    // Create placeholder proxies.
                    let (route_table_proxy, _server_end) = fidl::endpoints::create_proxy::<
                        <Ipv6 as FidlRouteAdminIpExt>::RouteTableMarker,
                    >();
                    let (route_set_proxy, _server_end) = fidl::endpoints::create_proxy::<
                        <Ipv6 as FidlRouteAdminIpExt>::RouteSetMarker,
                    >();

                    // It's expected that the netstack will ensure no-FIDL-ID clashes
                    // for us as long as we keep route table proxies alive.
                    let fidl_table_id = choose_unused_fidl_id(&*map);

                    map.insert(
                        netlink_id,
                        RouteTable::<Ipv6>::Managed(ManagedRouteTable {
                            route_table_proxy,
                            route_set_proxy,
                            fidl_table_id,
                            rule_set_authenticated: false,
                        }),
                    );

                    // Now the "reverse" lookup should yield the expected item.
                    assert_eq!(
                        map.get_netlink_id(&fidl_table_id),
                        Some(netlink_id)
                    );
                    // As should the "forward" lookup.
                    assert_eq!(
                        map.get(&netlink_id).map(|table| table.fidl_table_id()),
                        Some(fidl_table_id)
                    );
                };

            for op in ops_to_try {
                match op {
                    RouteTableMapOp::Insert(netlink_id) => {
                        test_insert(&mut route_table_map, netlink_id);
                    }
                    RouteTableMapOp::Remove(netlink_id) => {
                        test_remove(&mut route_table_map, netlink_id);
                    }
                }

                // After each op, check the invariant that the mapping between netlink IDs
                // and FIDL IDs is bidirectional.
                let by_route_table: Vec<(NetlinkRouteTableIndex, fnet_routes_ext::TableId)> =
                    route_table_map
                        .route_tables
                        .iter()
                        .map(|(netlink_id, table)| (*netlink_id, table.fidl_table_id()))
                        .sorted()
                        .collect::<Vec<_>>();

                let by_fidl_id: Vec<(NetlinkRouteTableIndex, fnet_routes_ext::TableId)> =
                    route_table_map
                        .fidl_table_ids
                        .iter()
                        .map(|(fidl_table_id, netlink_id)| (*netlink_id, *fidl_table_id))
                        .sorted()
                        .collect::<Vec<_>>();

                assert_eq!(by_route_table, by_fidl_id);
            }
        }

        #[test]
        fn fidl_route_map_maintains_invariants(ops: Vec<FidlRouteMapOp>) {
            let mut fidl_route_map = FidlRouteMap::default();

            for op in ops {
                match op {
                    FidlRouteMapOp::Add(TestRoute(route), table, props) => {
                        let table = fnet_routes_ext::TableId::from(table);
                        let props = fnet_routes_ext::EffectiveRouteProperties::from(props);

                        // Save copies before and after the addition.
                        let before_add = fidl_route_map.clone();
                        let prev_props = fidl_route_map.add(route, table, props);
                        let after_add = fidl_route_map.clone();

                        // Check idempotence.
                        let repeated_props = fidl_route_map.add(route, table, props);
                        prop_assert_eq!(&fidl_route_map, &after_add);
                        prop_assert_eq!(repeated_props, Some(props));

                        // Check that removal reverses addition if the table wasn't previously
                        // associated with the route.
                        if prev_props.is_none() {
                            let removed_props = fidl_route_map.remove(route, table);
                            prop_assert_eq!(&fidl_route_map, &before_add);
                            let p = assert_matches!(removed_props,
                                RouteRemoveResult::RemovedButTableNotEmpty(p)
                                | RouteRemoveResult::RemovedAndTableNewlyEmpty(p)
                                => p
                            );
                            prop_assert_eq!(p, props);

                            // Check that addition again has the same outcome.
                            prop_assert_eq!(fidl_route_map.add(route, table, props), None);
                            prop_assert_eq!(&fidl_route_map, &after_add);
                        }

                        // The route should register as installed.
                        prop_assert!(fidl_route_map
                            .route_is_installed_in_tables(&route, std::iter::once(&table)));
                        prop_assert!(!fidl_route_map
                            .route_is_uninstalled_in_tables(&route, std::iter::once(&table)));

                        // But still not register as installed in a table that doesn't exist.
                        prop_assert!(!fidl_route_map.route_is_installed_in_tables(
                            &route,
                            [&table, &NEVER_GENERATED_TABLE_ID]
                        ));

                        // But we still fail the "uninstalled" check even if there's another
                        // table it's not installed in.
                        prop_assert!(!fidl_route_map.route_is_uninstalled_in_tables(
                            &route,
                            [&table, &NEVER_GENERATED_TABLE_ID],
                        ));
                    }
                    FidlRouteMapOp::Remove(TestRoute(route), table) => {
                        let table = fnet_routes_ext::TableId::from(table);

                        // Save copies before and after the removal.
                        let before_remove = fidl_route_map.clone();
                        let prev_props = fidl_route_map.remove(route, table);
                        let after_remove = fidl_route_map.clone();

                        // Check idempotence.
                        prop_assert_eq!(
                            fidl_route_map.remove(route, table),
                            RouteRemoveResult::DidNotExist
                        );
                        assert_eq!(fidl_route_map, after_remove);

                        match prev_props {
                            RouteRemoveResult::DidNotExist => {
                                // If no properties were yielded on removal, the map should have
                                // remained the same.
                                prop_assert_eq!(before_remove, after_remove);
                            }
                            RouteRemoveResult::RemovedButTableNotEmpty(inner_prev_props)
                            | RouteRemoveResult::RemovedAndTableNewlyEmpty(inner_prev_props) => {
                                // If properties were yielded, then re-adding them should
                                // reverse the removal.
                                prop_assert_eq!(
                                    fidl_route_map.add(route, table, inner_prev_props),
                                    None
                                );
                                prop_assert_eq!(&fidl_route_map, &before_remove);

                                // Then removing the (route, table) association again should
                                // yield the same outcome.
                                let props = fidl_route_map.remove(route, table);
                                prop_assert_eq!(props, prev_props);
                                prop_assert_eq!(&fidl_route_map, &after_remove);
                            }
                        }

                        // The route should register as uninstalled.
                        prop_assert!(!fidl_route_map
                            .route_is_installed_in_tables(&route, std::iter::once(&table)));
                        prop_assert!(fidl_route_map
                            .route_is_uninstalled_in_tables(&route, std::iter::once(&table)));
                        prop_assert!(!fidl_route_map.route_is_installed_in_tables(
                            &route,
                            [&table, &NEVER_GENERATED_TABLE_ID]
                        ));
                        prop_assert!(fidl_route_map.route_is_uninstalled_in_tables(
                            &route,
                            [&table, &NEVER_GENERATED_TABLE_ID]
                        ));
                    }
                }

                // After each op, check that the map is consistent.
                let by_routes = fidl_route_map
                    .routes
                    .iter()
                    .flat_map(|(route, tables)| {
                        tables.iter().map(|(table, props)| (*route, *table, *props))
                    })
                    .sorted()
                    .collect::<Vec<_>>();
                let by_tables = fidl_route_map
                    .by_tables
                    .iter()
                    .flat_map(|(table, routes)| {
                        routes.iter().map(|(route, props)| (*route, *table, *props))
                    })
                    .sorted()
                    .collect::<Vec<_>>();
                prop_assert_eq!(by_routes, by_tables);
            }
        }
    }
}
