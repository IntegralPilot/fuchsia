// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! IPv6 Route Discovery as defined by [RFC 4861 section 6.3.4].
//!
//! [RFC 4861 section 6.3.4]: https://datatracker.ietf.org/doc/html/rfc4861#section-6.3.4

use core::hash::Hash;

use derivative::Derivative;
use log::debug;
use net_types::LinkLocalUnicastAddr;
use net_types::ip::{Ipv6Addr, Subnet};
use netstack3_base::{
    AnyDevice, CoreTimerContext, DeviceIdContext, HandleableTimer, InstantBindingsTypes,
    LocalTimerHeap, TimerBindingsTypes, TimerContext, TokenBucket, WeakDeviceIdentifier,
};
use netstack3_hashmap::HashMap;
use netstack3_hashmap::hash_map::Entry;
use packet_formats::icmp::ndp::NonZeroNdpLifetime;

use crate::internal::types::RoutePreference;

/// The maximum number of discovered routes the stack will track.
///
/// If new routes are discovered that would exceed this limit, they are ignored.
const MAX_DISCOVERED_ROUTES: usize = 4096;

/// The maximum number of routes that can be discovered per second.
///
/// If new routes are discovered that would exceed this limit, they are ignored.
const MAX_DISCOVERIES_PER_SECOND: u64 = 256;

/// Route discovery state on a device.
#[derive(Debug)]
pub struct Ipv6RouteDiscoveryState<BT: Ipv6RouteDiscoveryBindingsTypes> {
    // The valid (non-zero lifetime) discovered routes.
    //
    // Routes with a finite lifetime must have a timer set; routes with an
    // infinite lifetime must not.
    routes: HashMap<Ipv6DiscoveredRoute, Ipv6DiscoveredRouteProperties>,
    timers: LocalTimerHeap<Ipv6DiscoveredRoute, (), BT>,
    // Rate limit the discovery of routes. Add and update operations are rate
    // limited whereas delete operations are not. Delete operations are self
    // rate limiting, since they require the entry be present in the table.
    rate_limit: TokenBucket<BT::Instant>,
}

impl<BT: Ipv6RouteDiscoveryBindingsTypes> Ipv6RouteDiscoveryState<BT> {
    /// Gets the timer heap for route discovery.
    #[cfg(any(test, feature = "testutils"))]
    pub fn timers(&self) -> &LocalTimerHeap<Ipv6DiscoveredRoute, (), BT> {
        &self.timers
    }
}

impl<BC: Ipv6RouteDiscoveryBindingsContext> Ipv6RouteDiscoveryState<BC> {
    /// Constructs the route discovery state for `device_id`.
    pub fn new<D: WeakDeviceIdentifier, CC: CoreTimerContext<Ipv6DiscoveredRouteTimerId<D>, BC>>(
        bindings_ctx: &mut BC,
        device_id: D,
    ) -> Self {
        Self {
            routes: Default::default(),
            timers: LocalTimerHeap::new_with_context::<_, CC>(
                bindings_ctx,
                Ipv6DiscoveredRouteTimerId { device_id },
            ),
            rate_limit: TokenBucket::new(MAX_DISCOVERIES_PER_SECOND),
        }
    }
}

/// A discovered route.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct Ipv6DiscoveredRoute {
    /// The destination subnet for the route.
    pub subnet: Subnet<Ipv6Addr>,

    /// The next-hop node for the route, if required.
    ///
    /// `None` indicates that the subnet is on-link/directly-connected.
    pub gateway: Option<LinkLocalUnicastAddr<Ipv6Addr>>,
}

/// A discovered route's properties.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct Ipv6DiscoveredRouteProperties {
    /// The preference of the route.
    pub route_preference: RoutePreference,
}

/// A timer ID for IPv6 route discovery.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct Ipv6DiscoveredRouteTimerId<D: WeakDeviceIdentifier> {
    device_id: D,
}

impl<D: WeakDeviceIdentifier> Ipv6DiscoveredRouteTimerId<D> {
    pub(super) fn device_id(&self) -> &D {
        &self.device_id
    }
}

/// The configuration for route discovery.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Derivative)]
#[derivative(Default)]
pub struct RouteDiscoveryConfiguration {
    /// Allow default route to be added for this device.
    #[derivative(Default(value = "true"))]
    pub allow_default_route: bool,
}

/// The configuration update for route discovery.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]
#[allow(missing_docs)]
pub struct RouteDiscoveryConfigurationUpdate {
    pub allow_default_route: Option<bool>,
}

impl RouteDiscoveryConfiguration {
    /// Updates the route discovery configuration.
    ///
    /// Returns the previous value of the updated fields.
    pub fn update(
        &mut self,
        update: RouteDiscoveryConfigurationUpdate,
    ) -> RouteDiscoveryConfigurationUpdate {
        let RouteDiscoveryConfigurationUpdate { allow_default_route } = update;
        let allow_default_route =
            allow_default_route.map(|new| core::mem::replace(&mut self.allow_default_route, new));
        RouteDiscoveryConfigurationUpdate { allow_default_route }
    }
}

/// An implementation of the execution context available when accessing the IPv6
/// route discovery state.
///
/// See [`Ipv6RouteDiscoveryContext::with_discovered_routes_mut`].
pub trait Ipv6DiscoveredRoutesContext<BC>: DeviceIdContext<AnyDevice> {
    /// Adds a newly discovered IPv6 route to the routing table.
    fn add_discovered_ipv6_route(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        route: Ipv6DiscoveredRoute,
        properties: Ipv6DiscoveredRouteProperties,
    );

    /// Deletes a previously discovered (now invalidated) IPv6 route from the
    /// routing table.
    fn del_discovered_ipv6_route(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        route: Ipv6DiscoveredRoute,
    );
}

/// The execution context for IPv6 route discovery.
pub trait Ipv6RouteDiscoveryContext<BT: Ipv6RouteDiscoveryBindingsTypes>:
    DeviceIdContext<AnyDevice>
{
    /// The inner discovered routes context.
    type WithDiscoveredRoutesMutCtx<'a>: Ipv6DiscoveredRoutesContext<BT, DeviceId = Self::DeviceId>;

    /// Gets the route discovery state, mutably.
    fn with_discovered_routes_mut<
        O,
        F: FnOnce(&mut Ipv6RouteDiscoveryState<BT>, &mut Self::WithDiscoveredRoutesMutCtx<'_>) -> O,
    >(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;
}

/// The bindings types for IPv6 route discovery.
pub trait Ipv6RouteDiscoveryBindingsTypes: TimerBindingsTypes + InstantBindingsTypes {}
impl<BT> Ipv6RouteDiscoveryBindingsTypes for BT where BT: TimerBindingsTypes + InstantBindingsTypes {}

/// The bindings execution context for IPv6 route discovery.
pub trait Ipv6RouteDiscoveryBindingsContext:
    Ipv6RouteDiscoveryBindingsTypes + TimerContext
{
}
impl<BC> Ipv6RouteDiscoveryBindingsContext for BC where
    BC: Ipv6RouteDiscoveryBindingsTypes + TimerContext
{
}

/// An implementation of IPv6 route discovery.
pub trait RouteDiscoveryHandler<BC>: DeviceIdContext<AnyDevice> {
    /// Handles an update affecting discovered routes.
    ///
    /// A `None` value for `lifetime` indicates that the route is not valid and
    /// must be invalidated if it has been discovered; a `Some(_)` value
    /// indicates the new maximum lifetime that the route may be valid for
    /// before being invalidated.
    fn update_route(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        route: Ipv6DiscoveredRoute,
        properties: Ipv6DiscoveredRouteProperties,
        lifetime: Option<NonZeroNdpLifetime>,
        config: &RouteDiscoveryConfiguration,
    );

    /// Invalidates all discovered routes.
    fn invalidate_routes(&mut self, bindings_ctx: &mut BC, device_id: &Self::DeviceId);
}

impl<BC: Ipv6RouteDiscoveryBindingsContext, CC: Ipv6RouteDiscoveryContext<BC>>
    RouteDiscoveryHandler<BC> for CC
{
    fn update_route(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &CC::DeviceId,
        route: Ipv6DiscoveredRoute,
        properties: Ipv6DiscoveredRouteProperties,
        lifetime: Option<NonZeroNdpLifetime>,
        config: &RouteDiscoveryConfiguration,
    ) {
        self.with_discovered_routes_mut(device_id, |state, core_ctx| {
            let Ipv6RouteDiscoveryState { routes, timers, rate_limit } = state;
            match lifetime {
                Some(lifetime) => {
                    if !config.allow_default_route && route.subnet.prefix() == 0 {
                        return;
                    }
                    if !rate_limit.try_take(bindings_ctx) {
                        debug!(
                            "IPv6 Discovered routes rate limited. Ignoring update for {route:?}"
                        );
                        return;
                    }

                    let num_routes = routes.len();
                    let newly_added = match routes.entry(route) {
                        Entry::Occupied(mut entry) => {
                            let old_properties = entry.get_mut();
                            if old_properties.route_preference != properties.route_preference {
                                core_ctx.del_discovered_ipv6_route(bindings_ctx, device_id, route);
                                core_ctx.add_discovered_ipv6_route(
                                    bindings_ctx,
                                    device_id,
                                    route,
                                    properties,
                                );
                                *old_properties = properties;
                            }
                            false
                        }
                        Entry::Vacant(entry) => {
                            if num_routes >= MAX_DISCOVERED_ROUTES {
                                debug!(
                                    "IPv6 Discovered Routes table is full. Not adding {route:?}"
                                );
                                return;
                            }

                            core_ctx.add_discovered_ipv6_route(
                                bindings_ctx,
                                device_id,
                                route,
                                properties,
                            );
                            let _: &mut _ = entry.insert(properties);
                            true
                        }
                    };

                    let prev_timer_fires_at = match lifetime {
                        NonZeroNdpLifetime::Finite(lifetime) => {
                            timers.schedule_after(bindings_ctx, route, (), lifetime.get())
                        }
                        // Routes with an infinite lifetime have no timers.
                        NonZeroNdpLifetime::Infinite => timers.cancel(bindings_ctx, &route),
                    };

                    if newly_added {
                        if let Some((prev_timer_fires_at, ())) = prev_timer_fires_at {
                            panic!(
                                "newly added route {:?} should not have already been \
                                 scheduled to fire at {:?}",
                                route, prev_timer_fires_at,
                            )
                        }
                    }
                }
                None => {
                    if routes.remove(&route).is_some() {
                        invalidate_route(core_ctx, bindings_ctx, device_id, state, route);
                    }
                }
            }
        })
    }

    fn invalidate_routes(&mut self, bindings_ctx: &mut BC, device_id: &CC::DeviceId) {
        self.with_discovered_routes_mut(device_id, |state, core_ctx| {
            for (route, _properties) in core::mem::take(&mut state.routes).into_iter() {
                invalidate_route(core_ctx, bindings_ctx, device_id, state, route);
            }
        })
    }
}

impl<BC: Ipv6RouteDiscoveryBindingsContext, CC: Ipv6RouteDiscoveryContext<BC>>
    HandleableTimer<CC, BC> for Ipv6DiscoveredRouteTimerId<CC::WeakDeviceId>
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, _: BC::UniqueTimerId) {
        let Self { device_id } = self;
        let Some(device_id) = device_id.upgrade() else {
            return;
        };
        core_ctx.with_discovered_routes_mut(
            &device_id,
            |Ipv6RouteDiscoveryState { routes, timers, rate_limit: _ }, core_ctx| {
                let Some((route, ())) = timers.pop(bindings_ctx) else {
                    return;
                };
                let _properties =
                    routes.remove(&route).expect("invalidated route should be discovered");
                core_ctx.del_discovered_ipv6_route(bindings_ctx, &device_id, route);
            },
        )
    }
}

fn invalidate_route<BC: Ipv6RouteDiscoveryBindingsContext, CC: Ipv6DiscoveredRoutesContext<BC>>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    state: &mut Ipv6RouteDiscoveryState<BC>,
    route: Ipv6DiscoveredRoute,
) {
    // Routes with an infinite lifetime have no timers.
    let _: Option<(BC::Instant, ())> = state.timers.cancel(bindings_ctx, &route);
    core_ctx.del_discovered_ipv6_route(bindings_ctx, device_id, route)
}

#[cfg(test)]
mod tests {
    use netstack3_base::testutil::{
        FakeBindingsCtx, FakeCoreCtx, FakeDeviceId, FakeInstant, FakeTimerCtxExt as _,
        FakeWeakDeviceId,
    };
    use netstack3_base::{CtxPair, IntoCoreTimerCtx};
    use packet_formats::utils::NonZeroDuration;

    use super::*;
    use crate::internal::base::IPV6_DEFAULT_SUBNET;

    #[derive(Default)]
    struct FakeWithDiscoveredRoutesMutCtx {
        route_table: HashMap<Ipv6DiscoveredRoute, Ipv6DiscoveredRouteProperties>,
    }

    impl DeviceIdContext<AnyDevice> for FakeWithDiscoveredRoutesMutCtx {
        type DeviceId = FakeDeviceId;
        type WeakDeviceId = FakeWeakDeviceId<FakeDeviceId>;
    }

    impl<C> Ipv6DiscoveredRoutesContext<C> for FakeWithDiscoveredRoutesMutCtx {
        fn add_discovered_ipv6_route(
            &mut self,
            _bindings_ctx: &mut C,
            FakeDeviceId: &Self::DeviceId,
            route: Ipv6DiscoveredRoute,
            properties: Ipv6DiscoveredRouteProperties,
        ) {
            let Self { route_table } = self;
            let _: Option<Ipv6DiscoveredRouteProperties> = route_table.insert(route, properties);
        }

        fn del_discovered_ipv6_route(
            &mut self,
            _bindings_ctx: &mut C,
            FakeDeviceId: &Self::DeviceId,
            route: Ipv6DiscoveredRoute,
        ) {
            let Self { route_table } = self;
            let _: Option<Ipv6DiscoveredRouteProperties> = route_table.remove(&route);
        }
    }

    struct FakeIpv6RouteDiscoveryContext {
        state: Ipv6RouteDiscoveryState<FakeBindingsCtxImpl>,
        route_table: FakeWithDiscoveredRoutesMutCtx,
    }

    type FakeCoreCtxImpl = FakeCoreCtx<FakeIpv6RouteDiscoveryContext, (), FakeDeviceId>;

    type FakeBindingsCtxImpl =
        FakeBindingsCtx<Ipv6DiscoveredRouteTimerId<FakeWeakDeviceId<FakeDeviceId>>, (), (), ()>;

    impl Ipv6RouteDiscoveryContext<FakeBindingsCtxImpl> for FakeCoreCtxImpl {
        type WithDiscoveredRoutesMutCtx<'a> = FakeWithDiscoveredRoutesMutCtx;

        fn with_discovered_routes_mut<
            O,
            F: FnOnce(
                &mut Ipv6RouteDiscoveryState<FakeBindingsCtxImpl>,
                &mut Self::WithDiscoveredRoutesMutCtx<'_>,
            ) -> O,
        >(
            &mut self,
            &FakeDeviceId: &Self::DeviceId,
            cb: F,
        ) -> O {
            let FakeIpv6RouteDiscoveryContext { state, route_table, .. } = &mut self.state;
            cb(state, route_table)
        }
    }

    const ROUTE1: Ipv6DiscoveredRoute =
        Ipv6DiscoveredRoute { subnet: IPV6_DEFAULT_SUBNET, gateway: None };
    const PROP1: Ipv6DiscoveredRouteProperties =
        Ipv6DiscoveredRouteProperties { route_preference: RoutePreference::Medium };
    const ROUTE2: Ipv6DiscoveredRoute = Ipv6DiscoveredRoute {
        subnet: unsafe {
            Subnet::new_unchecked(Ipv6Addr::new([0x2620, 0x1012, 0x1000, 0x5000, 0, 0, 0, 0]), 64)
        },
        gateway: None,
    };
    const PROP2: Ipv6DiscoveredRouteProperties =
        Ipv6DiscoveredRouteProperties { route_preference: RoutePreference::High };

    const ONE_SECOND: NonZeroDuration = NonZeroDuration::from_secs(1).unwrap();
    const TWO_SECONDS: NonZeroDuration = NonZeroDuration::from_secs(2).unwrap();

    fn new_context() -> CtxPair<FakeCoreCtxImpl, FakeBindingsCtxImpl> {
        CtxPair::with_default_bindings_ctx(|bindings_ctx| {
            FakeCoreCtxImpl::with_state(FakeIpv6RouteDiscoveryContext {
                state: Ipv6RouteDiscoveryState::new::<_, IntoCoreTimerCtx>(
                    bindings_ctx,
                    FakeWeakDeviceId(FakeDeviceId),
                ),
                route_table: Default::default(),
            })
        })
    }

    #[test]
    fn new_route_no_lifetime() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context();

        RouteDiscoveryHandler::update_route(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeDeviceId,
            ROUTE1,
            PROP1,
            None,
            &Default::default(),
        );
        bindings_ctx.timers.assert_no_timers_installed();
    }

    fn discover_new_route(
        core_ctx: &mut FakeCoreCtxImpl,
        bindings_ctx: &mut FakeBindingsCtxImpl,
        route: Ipv6DiscoveredRoute,
        properties: Ipv6DiscoveredRouteProperties,
        duration: NonZeroNdpLifetime,
    ) {
        RouteDiscoveryHandler::update_route(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            route,
            properties,
            Some(duration),
            &Default::default(),
        );

        let route_table = &core_ctx.state.route_table.route_table;
        assert_eq!(route_table.get(&route), Some(&properties), "route_table={route_table:?}");

        let expect = match duration {
            NonZeroNdpLifetime::Finite(duration) => Some((FakeInstant::from(duration.get()), &())),
            NonZeroNdpLifetime::Infinite => None,
        };
        assert_eq!(core_ctx.state.state.timers.get(&route), expect);
    }

    fn trigger_next_timer(
        core_ctx: &mut FakeCoreCtxImpl,
        bindings_ctx: &mut FakeBindingsCtxImpl,
        route: Ipv6DiscoveredRoute,
    ) {
        core_ctx.state.state.timers.assert_top(&route, &());
        assert_eq!(
            bindings_ctx.trigger_next_timer(core_ctx),
            Some(Ipv6DiscoveredRouteTimerId { device_id: FakeWeakDeviceId(FakeDeviceId) })
        );
    }

    fn assert_route_invalidated(
        core_ctx: &mut FakeCoreCtxImpl,
        bindings_ctx: &mut FakeBindingsCtxImpl,
        route: Ipv6DiscoveredRoute,
    ) {
        let route_table = &core_ctx.state.route_table.route_table;
        assert!(!route_table.contains_key(&route), "route_table={route_table:?}");
        bindings_ctx.timers.assert_no_timers_installed();
    }

    fn assert_single_invalidation_timer(
        core_ctx: &mut FakeCoreCtxImpl,
        bindings_ctx: &mut FakeBindingsCtxImpl,
        route: Ipv6DiscoveredRoute,
    ) {
        trigger_next_timer(core_ctx, bindings_ctx, route);
        assert_route_invalidated(core_ctx, bindings_ctx, route);
    }

    #[test]
    fn invalidated_route_not_found() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context();

        discover_new_route(
            &mut core_ctx,
            &mut bindings_ctx,
            ROUTE1,
            PROP1,
            NonZeroNdpLifetime::Infinite,
        );

        // Fake the route already being removed from underneath the route
        // discovery table.
        assert!(core_ctx.state.route_table.route_table.remove(&ROUTE1).is_some());
        // Invalidating the route should ignore the fact that the route is not
        // in the route table.
        update_to_invalidate_check_invalidation(&mut core_ctx, &mut bindings_ctx, ROUTE1, PROP1);
    }

    #[test]
    fn new_route_with_infinite_lifetime() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context();

        discover_new_route(
            &mut core_ctx,
            &mut bindings_ctx,
            ROUTE1,
            PROP1,
            NonZeroNdpLifetime::Infinite,
        );
        bindings_ctx.timers.assert_no_timers_installed();
    }

    #[test]
    fn update_route_from_infinite_to_finite_lifetime() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context();

        discover_new_route(
            &mut core_ctx,
            &mut bindings_ctx,
            ROUTE1,
            PROP1,
            NonZeroNdpLifetime::Infinite,
        );
        bindings_ctx.timers.assert_no_timers_installed();

        RouteDiscoveryHandler::update_route(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeDeviceId,
            ROUTE1,
            PROP1,
            Some(NonZeroNdpLifetime::Finite(ONE_SECOND)),
            &Default::default(),
        );
        assert_eq!(
            core_ctx.state.state.timers.get(&ROUTE1),
            Some((FakeInstant::from(ONE_SECOND.get()), &()))
        );
        assert_single_invalidation_timer(&mut core_ctx, &mut bindings_ctx, ROUTE1);
    }

    fn update_to_invalidate_check_invalidation(
        core_ctx: &mut FakeCoreCtxImpl,
        bindings_ctx: &mut FakeBindingsCtxImpl,
        route: Ipv6DiscoveredRoute,
        properties: Ipv6DiscoveredRouteProperties,
    ) {
        RouteDiscoveryHandler::update_route(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            route,
            properties,
            None,
            &Default::default(),
        );
        assert_route_invalidated(core_ctx, bindings_ctx, route);
    }

    #[test]
    fn invalidate_route_with_infinite_lifetime() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context();

        discover_new_route(
            &mut core_ctx,
            &mut bindings_ctx,
            ROUTE1,
            PROP1,
            NonZeroNdpLifetime::Infinite,
        );
        bindings_ctx.timers.assert_no_timers_installed();

        update_to_invalidate_check_invalidation(&mut core_ctx, &mut bindings_ctx, ROUTE1, PROP1);
    }
    #[test]
    fn new_route_with_finite_lifetime() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context();

        discover_new_route(
            &mut core_ctx,
            &mut bindings_ctx,
            ROUTE1,
            PROP1,
            NonZeroNdpLifetime::Finite(ONE_SECOND),
        );
        assert_single_invalidation_timer(&mut core_ctx, &mut bindings_ctx, ROUTE1);
    }

    #[test]
    fn update_route_from_finite_to_infinite_lifetime() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context();

        discover_new_route(
            &mut core_ctx,
            &mut bindings_ctx,
            ROUTE1,
            PROP1,
            NonZeroNdpLifetime::Finite(ONE_SECOND),
        );

        RouteDiscoveryHandler::update_route(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeDeviceId,
            ROUTE1,
            PROP1,
            Some(NonZeroNdpLifetime::Infinite),
            &Default::default(),
        );
        bindings_ctx.timers.assert_no_timers_installed();
    }

    #[test]
    fn update_route_from_finite_to_finite_lifetime() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context();

        discover_new_route(
            &mut core_ctx,
            &mut bindings_ctx,
            ROUTE1,
            PROP1,
            NonZeroNdpLifetime::Finite(ONE_SECOND),
        );

        RouteDiscoveryHandler::update_route(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeDeviceId,
            ROUTE1,
            PROP1,
            Some(NonZeroNdpLifetime::Finite(TWO_SECONDS)),
            &Default::default(),
        );
        assert_eq!(
            core_ctx.state.state.timers.get(&ROUTE1),
            Some((FakeInstant::from(TWO_SECONDS.get()), &()))
        );
        assert_single_invalidation_timer(&mut core_ctx, &mut bindings_ctx, ROUTE1);
    }

    #[test]
    fn invalidate_route_with_finite_lifetime() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context();

        discover_new_route(
            &mut core_ctx,
            &mut bindings_ctx,
            ROUTE1,
            PROP1,
            NonZeroNdpLifetime::Finite(ONE_SECOND),
        );

        update_to_invalidate_check_invalidation(&mut core_ctx, &mut bindings_ctx, ROUTE1, PROP1);
    }

    #[test]
    fn invalidate_all_routes() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context();
        discover_new_route(
            &mut core_ctx,
            &mut bindings_ctx,
            ROUTE1,
            PROP1,
            NonZeroNdpLifetime::Finite(ONE_SECOND),
        );
        discover_new_route(
            &mut core_ctx,
            &mut bindings_ctx,
            ROUTE2,
            PROP2,
            NonZeroNdpLifetime::Finite(TWO_SECONDS),
        );

        RouteDiscoveryHandler::invalidate_routes(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId);
        bindings_ctx.timers.assert_no_timers_installed();
        let route_table = &core_ctx.state.route_table.route_table;
        assert!(route_table.is_empty(), "route_table={route_table:?}");
    }

    fn make_route(i: u16) -> Ipv6DiscoveredRoute {
        Ipv6DiscoveredRoute {
            subnet: Subnet::new(Ipv6Addr::new([0x2001, 0xdb8, 0, 0, 0, 0, 0, i]), 128)
                .expect("should be a valid IPv6 Subnet"),
            gateway: None,
        }
    }

    #[test]
    fn max_discovered_routes() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context();

        // Fill the routing table to the limit.
        for i in 0..MAX_DISCOVERED_ROUTES {
            let route = make_route(i as u16);
            discover_new_route(
                &mut core_ctx,
                &mut bindings_ctx,
                route,
                PROP1,
                NonZeroNdpLifetime::Infinite,
            );
            // NB: Advance the clock to avoid rate limiting in this test.
            bindings_ctx.timers.instant.sleep(core::time::Duration::from_secs(1));
        }
        assert_eq!(core_ctx.state.route_table.route_table.len(), MAX_DISCOVERED_ROUTES);

        // Try to add one more route, it should be ignored.
        let extra_route = make_route(MAX_DISCOVERED_ROUTES as u16);
        RouteDiscoveryHandler::update_route(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeDeviceId,
            extra_route,
            PROP1,
            Some(NonZeroNdpLifetime::Infinite),
            &Default::default(),
        );
        assert_eq!(core_ctx.state.route_table.route_table.len(), MAX_DISCOVERED_ROUTES);
        assert!(!core_ctx.state.route_table.route_table.contains_key(&extra_route));

        // Update an existing route, it should be allowed.
        let route_to_update = make_route(0);
        RouteDiscoveryHandler::update_route(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeDeviceId,
            route_to_update,
            PROP2,
            Some(NonZeroNdpLifetime::Infinite),
            &Default::default(),
        );
        assert_eq!(core_ctx.state.route_table.route_table.get(&route_to_update), Some(&PROP2));

        // Delete an existing route, it should be allowed.
        let route_to_delete = make_route(0);
        RouteDiscoveryHandler::update_route(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeDeviceId,
            route_to_delete,
            PROP2,
            None,
            &Default::default(),
        );
        assert_eq!(core_ctx.state.route_table.route_table.len(), MAX_DISCOVERED_ROUTES - 1);
        assert!(!core_ctx.state.route_table.route_table.contains_key(&route_to_delete));

        // Now we should be able to add the extra route.
        discover_new_route(
            &mut core_ctx,
            &mut bindings_ctx,
            extra_route,
            PROP1,
            NonZeroNdpLifetime::Infinite,
        );
        assert_eq!(core_ctx.state.route_table.route_table.len(), MAX_DISCOVERED_ROUTES);
        assert!(core_ctx.state.route_table.route_table.contains_key(&extra_route));
    }

    #[test]
    fn rate_limiting() {
        let CtxPair { mut core_ctx, mut bindings_ctx } = new_context();

        // We can add up to the limit routes immediately (initial burst).
        for i in 0..MAX_DISCOVERIES_PER_SECOND {
            let route = make_route(i as u16);
            discover_new_route(
                &mut core_ctx,
                &mut bindings_ctx,
                route,
                PROP1,
                NonZeroNdpLifetime::Infinite,
            );
        }
        assert_eq!(
            core_ctx.state.route_table.route_table.len(),
            MAX_DISCOVERIES_PER_SECOND as usize
        );

        // Rate limiting should prevent an additional route from being added.
        let extra_route = make_route(MAX_DISCOVERIES_PER_SECOND as u16);
        RouteDiscoveryHandler::update_route(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeDeviceId,
            extra_route,
            PROP1,
            Some(NonZeroNdpLifetime::Infinite),
            &Default::default(),
        );
        assert_eq!(
            core_ctx.state.route_table.route_table.len(),
            MAX_DISCOVERIES_PER_SECOND as usize
        );
        assert!(!core_ctx.state.route_table.route_table.contains_key(&extra_route));

        // Advance time by 1 second. The rate limit should be reset and
        // the add should succeed.
        bindings_ctx.timers.instant.sleep(core::time::Duration::from_secs(1));
        discover_new_route(
            &mut core_ctx,
            &mut bindings_ctx,
            extra_route,
            PROP1,
            NonZeroNdpLifetime::Infinite,
        );
        assert_eq!(
            core_ctx.state.route_table.route_table.len(),
            (MAX_DISCOVERIES_PER_SECOND + 1) as usize
        );
        assert!(core_ctx.state.route_table.route_table.contains_key(&extra_route));
    }
}
