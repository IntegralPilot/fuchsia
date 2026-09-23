// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Module for IP level paths' maximum transmission unit (PMTU) size
//! cache support.

use alloc::vec::Vec;
use core::time::Duration;

use log::trace;
use lru_cache::LruCache;
use net_types::ip::{GenericOverIp, Ip, IpAddress, IpVersionMarker, Mtu};
use netstack3_base::{
    CoreTimerContext, HandleableTimer, Instant, InstantBindingsTypes, TimerBindingsTypes,
    TimerContext,
};

/// Time between PMTU maintenance operations.
///
/// Maintenance operations are things like resetting cached PMTU data to force
/// restart PMTU discovery to detect increases in a PMTU.
///
/// 1 hour.
// TODO(ghanan): Make this value configurable by runtime options.
const MAINTENANCE_PERIOD: Duration = Duration::from_secs(3600);

/// Time for a PMTU value to be considered stale.
///
/// 3 hours.
// TODO(ghanan): Make this value configurable by runtime options.
const PMTU_STALE_TIMEOUT: Duration = Duration::from_secs(10800);

const MAX_ENTRIES: usize = 256;

/// The timer ID for the path MTU cache.
#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, Hash, GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct PmtuTimerId<I: Ip>(IpVersionMarker<I>);

/// The core context for the path MTU cache.
pub trait PmtuContext<I: Ip, BT: PmtuBindingsTypes> {
    /// Calls a function with a mutable reference to the PMTU cache.
    fn with_state_mut<O, F: FnOnce(&mut PmtuCache<I, BT>) -> O>(&mut self, cb: F) -> O;
}

/// The bindings types for path MTU discovery.
pub trait PmtuBindingsTypes: TimerBindingsTypes + InstantBindingsTypes {}
impl<BT> PmtuBindingsTypes for BT where BT: TimerBindingsTypes + InstantBindingsTypes {}

/// The bindings execution context for path MTU discovery.
trait PmtuBindingsContext: PmtuBindingsTypes + TimerContext {}
impl<BC> PmtuBindingsContext for BC where BC: PmtuBindingsTypes + TimerContext {}

/// A handler for incoming PMTU events.
///
/// `PmtuHandler` is intended to serve as the interface between ICMP the IP
/// layer, which holds the PMTU cache. In production, method calls are delegated
/// to a real [`PmtuCache`], while in testing, method calls may be delegated to
/// a fake implementation.
pub(crate) trait PmtuHandler<I: Ip, BC> {
    /// Updates the PMTU between `src_ip` and `dst_ip` if `new_mtu` is less than
    /// the current PMTU and does not violate the minimum MTU size requirements
    /// for an IP.
    ///
    /// Returns the current PMTU after the update, whether or not it was updated.
    /// `None` indicates that the PMTU was not updated and there was no existing
    /// value in the cache.
    //
    // TODO(https://fxbug.dev/383355972): consider enforcing an IP version-specific
    // minimum MTU with a type-safe MTU type.
    fn update_pmtu_if_less(
        &mut self,
        bindings_ctx: &mut BC,
        src_ip: I::Addr,
        dst_ip: I::Addr,
        new_mtu: Mtu,
    ) -> Option<Mtu>;

    /// Updates the PMTU between `src_ip` and `dst_ip` to the next lower
    /// estimate from `from`.
    ///
    /// Returns the current PMTU after the update, whether or not it was updated.
    /// `None` indicates that the PMTU was not updated and there was no existing
    /// value in the cache.
    //
    // TODO(https://fxbug.dev/383355972): consider enforcing an IP version-specific
    // minimum MTU with a type-safe MTU type.
    fn update_pmtu_next_lower(
        &mut self,
        bindings_ctx: &mut BC,
        src_ip: I::Addr,
        dst_ip: I::Addr,
        from: Mtu,
    ) -> Option<Mtu>;
}

fn maybe_schedule_timer<BC: PmtuBindingsContext>(
    bindings_ctx: &mut BC,
    timer: &mut BC::Timer,
    cache_is_empty: bool,
) {
    // Only attempt to create the next maintenance task if we still have
    // PMTU entries in the cache. If we don't, it would be a waste to
    // schedule the timer. We will let the next creation of a PMTU entry
    // create the timer.
    if cache_is_empty {
        return;
    }

    match bindings_ctx.scheduled_instant(timer) {
        Some(scheduled_at) => {
            let _: BC::Instant = scheduled_at;
            // Timer already set, nothing to do.
        }
        None => {
            // We only enter this match arm if a timer was not already set.
            assert_eq!(bindings_ctx.schedule_timer(MAINTENANCE_PERIOD, timer), None)
        }
    }
}

/// Returns the current MTU after an update to the cache.
fn handle_update_result<BC: PmtuBindingsContext>(
    bindings_ctx: &mut BC,
    timer: &mut BC::Timer,
    result: UpdateResult,
    cache_is_empty: bool,
) -> Option<Mtu> {
    match result {
        UpdateResult::Updated(new_mtu) => {
            maybe_schedule_timer(bindings_ctx, timer, cache_is_empty);
            Some(new_mtu)
        }
        // TODO(https://fxbug.dev/42174290): should we handle failure to update PMTU
        // differently?
        UpdateResult::NotUpdated(mtu) => mtu,
    }
}

impl<I: Ip, BC: PmtuBindingsContext, CC: PmtuContext<I, BC>> PmtuHandler<I, BC> for CC {
    fn update_pmtu_if_less(
        &mut self,
        bindings_ctx: &mut BC,
        src_ip: I::Addr,
        dst_ip: I::Addr,
        new_mtu: Mtu,
    ) -> Option<Mtu> {
        self.with_state_mut(|cache| {
            let now = bindings_ctx.now();
            let res = cache.update_pmtu_if_less(src_ip, dst_ip, new_mtu, now);
            let is_empty = cache.is_empty();
            handle_update_result(bindings_ctx, &mut cache.timer, res, is_empty)
        })
    }

    fn update_pmtu_next_lower(
        &mut self,
        bindings_ctx: &mut BC,
        src_ip: I::Addr,
        dst_ip: I::Addr,
        from: Mtu,
    ) -> Option<Mtu> {
        self.with_state_mut(|cache| {
            let now = bindings_ctx.now();
            let res = cache.update_pmtu_next_lower(src_ip, dst_ip, from, now);
            let is_empty = cache.is_empty();
            handle_update_result(bindings_ctx, &mut cache.timer, res, is_empty)
        })
    }
}

impl<I: Ip, BC: PmtuBindingsContext, CC: PmtuContext<I, BC>> HandleableTimer<CC, BC>
    for PmtuTimerId<I>
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, _: BC::UniqueTimerId) {
        let Self(IpVersionMarker { .. }) = self;
        core_ctx.with_state_mut(|cache| {
            let now = bindings_ctx.now();
            cache.handle_timer(now);
            let is_empty = cache.is_empty();
            maybe_schedule_timer(bindings_ctx, &mut cache.timer, is_empty);
        })
    }
}

/// The key used to identify a path.
///
/// This is a tuple of (src_ip, dst_ip) as a path is only identified by the
/// source and destination addresses.
// TODO(ghanan): Should device play a part in the key-ing of a path?
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct PmtuCacheKey<A: IpAddress>(A, A);

impl<A: IpAddress> PmtuCacheKey<A> {
    fn new(src_ip: A, dst_ip: A) -> Self {
        Self(src_ip, dst_ip)
    }
}

/// IP layer PMTU cache data.
#[derive(Debug, PartialEq)]
pub(crate) struct PmtuCacheData<I> {
    pmtu: Mtu,
    last_updated: I,
}

impl<I: Instant> PmtuCacheData<I> {
    /// Construct a new `PmtuCacheData`.
    ///
    /// `last_updated` will be set to `now`.
    fn new(pmtu: Mtu, now: I) -> Self {
        Self { pmtu, last_updated: now }
    }
}

/// A path MTU cache.
pub struct PmtuCache<I: Ip, BT: PmtuBindingsTypes> {
    cache: LruCache<PmtuCacheKey<I::Addr>, PmtuCacheData<BT::Instant>>,
    timer: BT::Timer,
}

impl<I: Ip, BC: PmtuBindingsTypes + TimerContext> PmtuCache<I, BC> {
    pub(crate) fn new<CC: CoreTimerContext<PmtuTimerId<I>, BC>>(bindings_ctx: &mut BC) -> Self {
        Self {
            cache: LruCache::new(MAX_ENTRIES),
            timer: CC::new_timer(bindings_ctx, PmtuTimerId::default()),
        }
    }
}

enum UpdateResult {
    Updated(Mtu),
    NotUpdated(Option<Mtu>),
}

impl<I: Ip, BT: PmtuBindingsTypes> PmtuCache<I, BT> {
    /// Gets the PMTU between `src_ip` and `dst_ip`.
    pub fn get_pmtu(&mut self, src_ip: I::Addr, dst_ip: I::Addr) -> Option<Mtu> {
        self.cache.get_mut(&PmtuCacheKey::new(src_ip, dst_ip)).map(|x| x.pmtu)
    }

    /// Updates the PMTU between `src_ip` and `dst_ip` if `new_mtu` is less than the
    /// current PMTU and does not violate the minimum MTU size requirements for an
    /// IP.
    ///
    /// Returns the PMTU after updating the cache. Note that this could be `None` if
    /// the cache was previously empty and the new PMTU was invalid, and could also
    /// be a value that was already in the cache.
    fn update_pmtu_if_less(
        &mut self,
        src_ip: I::Addr,
        dst_ip: I::Addr,
        new_mtu: Mtu,
        now: BT::Instant,
    ) -> UpdateResult {
        match self.get_pmtu(src_ip, dst_ip) {
            // No PMTU exists so update.
            None => self.update_pmtu(src_ip, dst_ip, new_mtu, now),
            // A PMTU exists but it is greater than `new_mtu` so update.
            Some(prev_mtu) if new_mtu < prev_mtu => self.update_pmtu(src_ip, dst_ip, new_mtu, now),
            // A PMTU exists but it is less than or equal to `new_mtu` so no need to
            // update.
            Some(prev_mtu) => {
                trace!(
                    "update_pmtu_if_less: Not updating the PMTU between src {src_ip} and dst
                    {dst_ip} to {new_mtu:?}; is {prev_mtu:?}"
                );
                UpdateResult::NotUpdated(Some(prev_mtu))
            }
        }
    }

    /// Updates the PMTU between `src_ip` and `dst_ip` to the next lower
    /// estimate from `from`.
    ///
    /// Returns `Ok(x)` on successful update (either PMTU is already lower than
    /// `from`, in which case `x` is `None`, or a lower PMTU value exists that does
    /// not violate IP specific minimum MTU requirements and it is less than the
    /// current PMTU estimate, in which case `x` is `Some(a)` where `a` is the new
    /// lower value.
    ///
    /// Returns `Err(x)` if no suitable lower PMTU value exists, where `x` is the
    /// existing PMTU in the cache.
    fn update_pmtu_next_lower(
        &mut self,
        src_ip: I::Addr,
        dst_ip: I::Addr,
        from: Mtu,
        now: BT::Instant,
    ) -> UpdateResult {
        if let Some(next_pmtu) = next_lower_pmtu_plateau(from) {
            trace!(
                "update_pmtu_next_lower: Attempting to update PMTU between src {src_ip} and dst \
                {dst_ip} to {next_pmtu:?}"
            );

            self.update_pmtu_if_less(src_ip, dst_ip, next_pmtu, now)
        } else {
            // TODO(https://fxbug.dev/383355972): Should we make sure the current PMTU value
            // is set to the IP specific minimum MTU value?
            trace!(
                "update_pmtu_next_lower: Not updating PMTU between src {src_ip} and dst {dst_ip} \
                as there is no lower PMTU value from {from:?}"
            );
            UpdateResult::NotUpdated(self.get_pmtu(src_ip, dst_ip))
        }
    }

    /// Updates the PMTU between `src_ip` and `dst_ip` if `new_mtu` does not violate
    /// IP-specific minimum MTU requirements.
    ///
    /// Returns the PMTU after updating the cache. Note that this could be `None` if
    /// the cache was previously empty and the new PMTU was invalid, and could also
    /// be a value that was already in the cache.
    fn update_pmtu(
        &mut self,
        src_ip: I::Addr,
        dst_ip: I::Addr,
        new_mtu: Mtu,
        now: BT::Instant,
    ) -> UpdateResult {
        // New MTU must not be smaller than the minimum MTU for an IP.
        //
        // TODO(https://fxbug.dev/383355972): consider enforcing this invariant with a
        // type-safe MTU type that forces the caller to provide a valid MTU for the
        // given IP version.
        if new_mtu < I::MINIMUM_LINK_MTU {
            return UpdateResult::NotUpdated(self.get_pmtu(src_ip, dst_ip));
        }
        let _previous =
            self.cache.insert(PmtuCacheKey::new(src_ip, dst_ip), PmtuCacheData::new(new_mtu, now));

        log::debug!("updated PMTU for path {src_ip} -> {dst_ip} to {new_mtu:?}");

        UpdateResult::Updated(new_mtu)
    }

    fn handle_timer(&mut self, now: BT::Instant) {
        // Make sure we expected this timer to fire.
        assert!(!self.cache.is_empty());

        // Remove all stale PMTU data to force restart the PMTU discovery
        // process. This will be ok because the next time we try to send a
        // packet to some node, we will update the PMTU with the first known
        // potential PMTU (the first link's (connected to the node attempting
        // PMTU discovery)) PMTU.
        //
        // TODO(ghanan): Add per-path options as per RFC 1981 section 5.3.
        //               Specifically, some links/paths may not need to have
        //               PMTU rediscovered as the PMTU will never change.
        //
        // TODO(ghanan): Consider not simply deleting all stale PMTU data as
        //               this may cause packets to be dropped every time the
        //               data seems to get stale when really it is still
        //               valid. Considering the use case, PMTU value changes
        //               may be infrequent so it may be enough to just use a
        //               long stale timer.
        //
        // TODO(https://fxbug.dev/404629697): once we actually use the PMTU
        // cache to inform IP fragmentation, consider discarding least-recently-
        // used entries rather than, or in addition to, entries that have been
        // in the cache for a long time.
        //
        // TODO(https://fxbug.dev/406779050): use `LruCache::retain` when such a
        // method is available to avoid allocating a separate `Vec` of entries
        // to remove.
        let to_remove: Vec<_> = self
            .cache
            .iter()
            .filter_map(|(k, v)| {
                (now.saturating_duration_since(v.last_updated) >= PMTU_STALE_TIMEOUT).then_some(*k)
            })
            .collect();
        for key in to_remove {
            let _: Option<_> = self.cache.remove(&key);
        }
    }

    fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

/// Get next lower PMTU plateau value, if one exists.
fn next_lower_pmtu_plateau(start_mtu: Mtu) -> Option<Mtu> {
    /// Common MTU values taken from [RFC 1191 section 7.1].
    ///
    /// This list includes lower bounds of groups of common MTU values that are
    /// relatively close to each other, sorted in descending order.
    ///
    /// Note, the RFC does not actually include the value 1280 in the list of
    /// plateau values, but we include it here because it is the minimum IPv6
    /// MTU value and is not expected to be an uncommon value for MTUs.
    ///
    /// This list MUST be sorted in descending order; methods such as
    /// `next_lower_pmtu_plateau` assume `PMTU_PLATEAUS` has this property.
    ///
    /// We use this list when estimating PMTU values when doing PMTU discovery
    /// with IPv4 on paths with nodes that do not implement RFC 1191. This list
    /// is useful as in practice, relatively few MTU values are in use.
    ///
    /// [RFC 1191 section 7.1]: https://tools.ietf.org/html/rfc1191#section-7.1
    const PMTU_PLATEAUS: [Mtu; 12] = [
        Mtu::new(65535),
        Mtu::new(32000),
        Mtu::new(17914),
        Mtu::new(8166),
        Mtu::new(4352),
        Mtu::new(2002),
        Mtu::new(1492),
        Mtu::new(1280),
        Mtu::new(1006),
        Mtu::new(508),
        Mtu::new(296),
        Mtu::new(68),
    ];

    for i in 0..PMTU_PLATEAUS.len() {
        let pmtu = PMTU_PLATEAUS[i];

        if pmtu < start_mtu {
            // Current PMTU is less than `start_mtu` and we know `PMTU_PLATEAUS`
            // is sorted so this is the next best PMTU estimate.
            return Some(pmtu);
        }
    }

    None
}

#[cfg(test)]
#[macro_use]
pub(crate) mod testutil {
    /// Implement the `PmtuHandler<$ip_version>` trait by just panicking.
    macro_rules! impl_pmtu_handler {
        ($ty:ty, $ctx:ty, $ip_version:ident) => {
            impl PmtuHandler<net_types::ip::$ip_version, $ctx> for $ty {
                fn update_pmtu_if_less(
                    &mut self,
                    _ctx: &mut $ctx,
                    _src_ip: <net_types::ip::$ip_version as net_types::ip::Ip>::Addr,
                    _dst_ip: <net_types::ip::$ip_version as net_types::ip::Ip>::Addr,
                    _new_mtu: Mtu,
                ) -> Option<Mtu> {
                    unimplemented!()
                }

                fn update_pmtu_next_lower(
                    &mut self,
                    _ctx: &mut $ctx,
                    _src_ip: <net_types::ip::$ip_version as net_types::ip::Ip>::Addr,
                    _dst_ip: <net_types::ip::$ip_version as net_types::ip::Ip>::Addr,
                    _from: Mtu,
                ) -> Option<Mtu> {
                    unimplemented!()
                }
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ip_test_macro::ip_test;
    use net_types::{SpecifiedAddr, Witness};
    use netstack3_base::testutil::{
        FakeBindingsCtx, FakeCoreCtx, FakeInstant, FakeTimerCtxExt, TestIpExt, assert_empty,
    };
    use netstack3_base::{CtxPair, InstantContext, IntoCoreTimerCtx};
    use test_case::test_case;

    struct FakePmtuContext<I: Ip> {
        cache: PmtuCache<I, FakeBindingsCtxImpl<I>>,
    }

    type FakeCtxImpl<I> = CtxPair<FakeCoreCtxImpl<I>, FakeBindingsCtxImpl<I>>;
    type FakeCoreCtxImpl<I> = FakeCoreCtx<FakePmtuContext<I>, (), ()>;
    type FakeBindingsCtxImpl<I> = FakeBindingsCtx<PmtuTimerId<I>, (), (), ()>;

    impl<I: Ip> PmtuContext<I, FakeBindingsCtxImpl<I>> for FakeCoreCtxImpl<I> {
        fn with_state_mut<O, F: FnOnce(&mut PmtuCache<I, FakeBindingsCtxImpl<I>>) -> O>(
            &mut self,
            cb: F,
        ) -> O {
            cb(&mut self.state.cache)
        }
    }

    fn new_context<I: Ip>() -> FakeCtxImpl<I> {
        FakeCtxImpl::with_default_bindings_ctx(|bindings_ctx| {
            FakeCoreCtxImpl::with_state(FakePmtuContext {
                cache: PmtuCache::new::<IntoCoreTimerCtx>(bindings_ctx),
            })
        })
    }

    /// Get an IPv4 or IPv6 address within the same subnet as that of
    /// `TEST_ADDRS_*`, but with the last octet set to `3`.
    fn get_other_ip_address<I: TestIpExt>() -> SpecifiedAddr<I::Addr> {
        I::get_other_ip_address(3)
    }

    impl<I: Ip, BT: PmtuBindingsTypes> PmtuCache<I, BT> {
        /// Gets the last updated [`Instant`] when the PMTU between `src_ip` and
        /// `dst_ip` was updated.
        ///
        /// [`Instant`]: Instant
        fn get_last_updated(&mut self, src_ip: I::Addr, dst_ip: I::Addr) -> Option<BT::Instant> {
            self.cache.get_mut(&PmtuCacheKey::new(src_ip, dst_ip)).map(|x| x.last_updated.clone())
        }
    }

    #[test_case(Mtu::new(65536) => Some(Mtu::new(65535)))]
    #[test_case(Mtu::new(65535) => Some(Mtu::new(32000)))]
    #[test_case(Mtu::new(65534) => Some(Mtu::new(32000)))]
    #[test_case(Mtu::new(32001) => Some(Mtu::new(32000)))]
    #[test_case(Mtu::new(32000) => Some(Mtu::new(17914)))]
    #[test_case(Mtu::new(31999) => Some(Mtu::new(17914)))]
    #[test_case(Mtu::new(1281)  => Some(Mtu::new(1280)))]
    #[test_case(Mtu::new(1280)  => Some(Mtu::new(1006)))]
    #[test_case(Mtu::new(69)    => Some(Mtu::new(68)))]
    #[test_case(Mtu::new(68)    => None)]
    #[test_case(Mtu::new(67)    => None)]
    #[test_case(Mtu::new(0)     => None)]
    fn test_next_lower_pmtu_plateau(start: Mtu) -> Option<Mtu> {
        next_lower_pmtu_plateau(start)
    }

    fn get_pmtu<I: Ip>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        src_ip: I::Addr,
        dst_ip: I::Addr,
    ) -> Option<Mtu> {
        core_ctx.state.cache.get_pmtu(src_ip, dst_ip)
    }

    fn get_last_updated<I: Ip>(
        core_ctx: &mut FakeCoreCtxImpl<I>,
        src_ip: I::Addr,
        dst_ip: I::Addr,
    ) -> Option<FakeInstant> {
        core_ctx.state.cache.get_last_updated(src_ip, dst_ip)
    }

    #[ip_test(I)]
    fn test_ip_path_mtu_cache_ctx<I: TestIpExt>() {
        let fake_config = I::TEST_ADDRS;
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Nothing in the cache yet
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get()),
            None
        );
        assert_eq!(
            get_last_updated(
                &mut core_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get()
            ),
            None
        );

        let new_mtu1 = Mtu::new(u32::from(I::MINIMUM_LINK_MTU) + 50);
        let start_time = bindings_ctx.now();
        let duration = Duration::from_secs(1);

        // Advance time to 1s.
        assert_empty(bindings_ctx.trigger_timers_for(duration, &mut core_ctx));

        // Update pmtu from local to remote. PMTU should be updated to
        // `new_mtu1` and last updated instant should be updated to the start of
        // the test + 1s.
        assert_eq!(
            PmtuHandler::update_pmtu_if_less(
                &mut core_ctx,
                &mut bindings_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get(),
                new_mtu1,
            ),
            Some(new_mtu1)
        );

        // Advance time to 2s.
        assert_empty(bindings_ctx.trigger_timers_for(duration, &mut core_ctx));

        // Make sure the update worked. PMTU should be updated to `new_mtu1` and
        // last updated instant should be updated to the start of the test + 1s
        // (when the update occurred.
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
                .unwrap(),
            new_mtu1
        );
        assert_eq!(
            get_last_updated(
                &mut core_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get()
            )
            .unwrap(),
            start_time + duration
        );

        let new_mtu2 = Mtu::new(u32::from(new_mtu1) - 1);

        // Advance time to 3s.
        assert_empty(bindings_ctx.trigger_timers_for(duration, &mut core_ctx));

        // Updating again should return the last pmtu PMTU should be updated to
        // `new_mtu2` and last updated instant should be updated to the start of
        // the test + 3s.
        assert_eq!(
            PmtuHandler::update_pmtu_if_less(
                &mut core_ctx,
                &mut bindings_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get(),
                new_mtu2,
            ),
            Some(new_mtu2)
        );

        // Advance time to 4s.
        assert_empty(bindings_ctx.trigger_timers_for(duration, &mut core_ctx));

        // Make sure the update worked. PMTU should be updated to `new_mtu2` and
        // last updated instant should be updated to the start of the test + 3s
        // (when the update occurred).
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
                .unwrap(),
            new_mtu2
        );
        assert_eq!(
            get_last_updated(
                &mut core_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get()
            )
            .unwrap(),
            start_time + (duration * 3)
        );

        let new_mtu3 = Mtu::new(u32::from(new_mtu2) - 1);

        // Advance time to 5s.
        assert_empty(bindings_ctx.trigger_timers_for(duration, &mut core_ctx));

        // Make sure update only if new PMTU is less than current (it is). PMTU
        // should be updated to `new_mtu3` and last updated instant should be
        // updated to the start of the test + 5s.
        assert_eq!(
            PmtuHandler::update_pmtu_if_less(
                &mut core_ctx,
                &mut bindings_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get(),
                new_mtu3,
            ),
            Some(new_mtu3)
        );

        // Advance time to 6s.
        assert_empty(bindings_ctx.trigger_timers_for(duration, &mut core_ctx));

        // Make sure the update worked. PMTU should be updated to `new_mtu3` and
        // last updated instant should be updated to the start of the test + 5s
        // (when the update occurred).
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
                .unwrap(),
            new_mtu3
        );
        let last_updated = start_time + (duration * 5);
        assert_eq!(
            get_last_updated(
                &mut core_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get()
            )
            .unwrap(),
            last_updated
        );

        let new_mtu4 = Mtu::new(u32::from(new_mtu3) + 50);

        // Advance time to 7s.
        assert_empty(bindings_ctx.trigger_timers_for(duration, &mut core_ctx));

        // Make sure update only if new PMTU is less than current (it isn't)
        assert_eq!(
            PmtuHandler::update_pmtu_if_less(
                &mut core_ctx,
                &mut bindings_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get(),
                new_mtu4,
            ),
            Some(new_mtu3)
        );

        // Advance time to 8s.
        assert_empty(bindings_ctx.trigger_timers_for(duration, &mut core_ctx));

        // Make sure the update didn't work. PMTU and last updated should not
        // have changed.
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
                .unwrap(),
            new_mtu3
        );
        assert_eq!(
            get_last_updated(
                &mut core_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get()
            )
            .unwrap(),
            last_updated
        );

        let low_mtu = Mtu::new(u32::from(I::MINIMUM_LINK_MTU) - 1);

        // Advance time to 9s.
        assert_empty(bindings_ctx.trigger_timers_for(duration, &mut core_ctx));

        // Updating with MTU value less than the minimum MTU should fail.
        assert_eq!(
            PmtuHandler::update_pmtu_if_less(
                &mut core_ctx,
                &mut bindings_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get(),
                low_mtu,
            ),
            Some(new_mtu3)
        );

        // Advance time to 10s.
        assert_empty(bindings_ctx.trigger_timers_for(duration, &mut core_ctx));

        // Make sure the update didn't work. PMTU and last updated should not
        // have changed.
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
                .unwrap(),
            new_mtu3
        );
        assert_eq!(
            get_last_updated(
                &mut core_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get()
            )
            .unwrap(),
            last_updated
        );
    }

    #[ip_test(I)]
    fn test_ip_pmtu_task<I: TestIpExt>() {
        let fake_config = I::TEST_ADDRS;
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Make sure there are no timers.
        bindings_ctx.timers.assert_no_timers_installed();

        let new_mtu1 = Mtu::new(u32::from(I::MINIMUM_LINK_MTU) + 50);
        let start_time = bindings_ctx.now();
        let duration = Duration::from_secs(1);

        // Advance time to 1s.
        assert_empty(bindings_ctx.trigger_timers_for(duration, &mut core_ctx));

        // Update pmtu from local to remote. PMTU should be updated to
        // `new_mtu1` and last updated instant should be updated to the start of
        // the test + 1s.
        assert_eq!(
            PmtuHandler::update_pmtu_if_less(
                &mut core_ctx,
                &mut bindings_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get(),
                new_mtu1,
            ),
            Some(new_mtu1)
        );

        // Make sure a task got scheduled.
        bindings_ctx.timers.assert_timers_installed([(
            PmtuTimerId::default(),
            FakeInstant::from(MAINTENANCE_PERIOD + Duration::from_secs(1)),
        )]);

        // Advance time to 2s.
        assert_empty(bindings_ctx.trigger_timers_for(duration, &mut core_ctx));

        // Make sure the update worked. PMTU should be updated to `new_mtu1` and
        // last updated instant should be updated to the start of the test + 1s
        // (when the update occurred.
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
                .unwrap(),
            new_mtu1
        );
        assert_eq!(
            get_last_updated(
                &mut core_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get()
            )
            .unwrap(),
            start_time + duration
        );

        // Advance time to 30mins.
        assert_empty(bindings_ctx.trigger_timers_for(duration * 1798, &mut core_ctx));

        // Update pmtu from local to another remote. PMTU should be updated to
        // `new_mtu1` and last updated instant should be updated to the start of
        // the test + 1s.
        let other_ip = get_other_ip_address::<I>();
        let new_mtu2 = Mtu::new(u32::from(I::MINIMUM_LINK_MTU) + 100);
        assert_eq!(
            PmtuHandler::update_pmtu_if_less(
                &mut core_ctx,
                &mut bindings_ctx,
                fake_config.local_ip.get(),
                other_ip.get(),
                new_mtu2,
            ),
            Some(new_mtu2)
        );

        // Make sure there is still a task scheduled. (we know no timers got
        // triggered because the `run_for` methods returned 0 so far).
        bindings_ctx.timers.assert_timers_installed([(
            PmtuTimerId::default(),
            FakeInstant::from(MAINTENANCE_PERIOD + Duration::from_secs(1)),
        )]);

        // Make sure the update worked. PMTU should be updated to `new_mtu2` and
        // last updated instant should be updated to the start of the test +
        // 30mins + 2s (when the update occurred.
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), other_ip.get()).unwrap(),
            new_mtu2
        );
        assert_eq!(
            get_last_updated(&mut core_ctx, fake_config.local_ip.get(), other_ip.get()).unwrap(),
            start_time + (duration * 1800)
        );
        // Make sure first update is still in the cache.
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
                .unwrap(),
            new_mtu1
        );
        assert_eq!(
            get_last_updated(
                &mut core_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get()
            )
            .unwrap(),
            start_time + duration
        );

        // Advance time to 1hr + 1s. Should have triggered a timer.
        bindings_ctx.trigger_timers_for_and_expect(
            duration * 1801,
            [PmtuTimerId::default()],
            &mut core_ctx,
        );
        // Make sure none of the cache data has been marked as stale and
        // removed.
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get())
                .unwrap(),
            new_mtu1
        );
        assert_eq!(
            get_last_updated(
                &mut core_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get()
            )
            .unwrap(),
            start_time + duration
        );
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), other_ip.get()).unwrap(),
            new_mtu2
        );
        assert_eq!(
            get_last_updated(&mut core_ctx, fake_config.local_ip.get(), other_ip.get()).unwrap(),
            start_time + (duration * 1800)
        );
        // Should still have another task scheduled.
        bindings_ctx.timers.assert_timers_installed([(
            PmtuTimerId::default(),
            FakeInstant::from(MAINTENANCE_PERIOD * 2 + Duration::from_secs(1)),
        )]);

        // Advance time to 3hr + 1s. Should have triggered 2 timers.
        bindings_ctx.trigger_timers_for_and_expect(
            duration * 7200,
            [PmtuTimerId::default(), PmtuTimerId::default()],
            &mut core_ctx,
        );
        // Make sure only the earlier PMTU data got marked as stale and removed.
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get()),
            None
        );
        assert_eq!(
            get_last_updated(
                &mut core_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get()
            ),
            None
        );
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), other_ip.get()).unwrap(),
            new_mtu2
        );
        assert_eq!(
            get_last_updated(&mut core_ctx, fake_config.local_ip.get(), other_ip.get()).unwrap(),
            start_time + (duration * 1800)
        );
        // Should still have another task scheduled.
        bindings_ctx.timers.assert_timers_installed([(
            PmtuTimerId::default(),
            FakeInstant::from(MAINTENANCE_PERIOD * 4 + Duration::from_secs(1)),
        )]);

        // Advance time to 4hr + 1s. Should have triggered 1 timers.
        bindings_ctx.trigger_timers_for_and_expect(
            duration * 3600,
            [PmtuTimerId::default()],
            &mut core_ctx,
        );
        // Make sure both PMTU data got marked as stale and removed.
        assert_eq!(
            get_pmtu(&mut core_ctx, fake_config.local_ip.get(), fake_config.remote_ip.get()),
            None
        );
        assert_eq!(
            get_last_updated(
                &mut core_ctx,
                fake_config.local_ip.get(),
                fake_config.remote_ip.get()
            ),
            None
        );
        assert_eq!(get_pmtu(&mut core_ctx, fake_config.local_ip.get(), other_ip.get()), None);
        assert_eq!(
            get_last_updated(&mut core_ctx, fake_config.local_ip.get(), other_ip.get()),
            None
        );
        // Should not have a task scheduled since there is no more PMTU data.
        bindings_ctx.timers.assert_no_timers_installed();
    }

    #[ip_test(I)]
    fn discard_lru<I: TestIpExt>() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        // Fill the cache to capacity.
        //
        // If this assertion trips because we've increased `MAX_ENTRIES`, we'll need to
        // update this test to use a different method than `get_other_ip_address` since
        // it only allows us to choose a single byte of the address.
        assert!(MAX_ENTRIES <= usize::from(u8::MAX) + 1);
        for i in 0..MAX_ENTRIES {
            let i = u8::try_from(i).unwrap();
            assert_eq!(
                PmtuHandler::update_pmtu_if_less(
                    &mut core_ctx,
                    &mut bindings_ctx,
                    *I::TEST_ADDRS.local_ip,
                    *I::get_other_ip_address(i),
                    Mtu::max(),
                ),
                Some(Mtu::max())
            );
        }
        assert_eq!(core_ctx.state.cache.cache.len(), MAX_ENTRIES);

        // The next insertion should cause the LRU entry to be discarded.
        assert_eq!(
            PmtuHandler::update_pmtu_if_less(
                &mut core_ctx,
                &mut bindings_ctx,
                *I::TEST_ADDRS.remote_ip,
                *I::TEST_ADDRS.local_ip,
                Mtu::max(),
            ),
            Some(Mtu::max())
        );
        assert_eq!(core_ctx.state.cache.cache.len(), MAX_ENTRIES);
        assert_eq!(
            core_ctx.state.cache.get_pmtu(*I::TEST_ADDRS.local_ip, *I::get_other_ip_address(0)),
            None
        );
        assert_eq!(
            core_ctx.state.cache.get_pmtu(*I::TEST_ADDRS.remote_ip, *I::TEST_ADDRS.local_ip),
            Some(Mtu::max())
        );
    }
}
