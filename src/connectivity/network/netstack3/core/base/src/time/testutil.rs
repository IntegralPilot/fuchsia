// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Test utilities for dealing with time.

use alloc::collections::BinaryHeap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Debug, Formatter};
use core::hash::Hash;
use core::ops;
use core::sync::atomic::Ordering;
use core::time::Duration;
use netstack3_sync::Mutex;

use assert_matches::assert_matches;
use netstack3_hashmap::HashMap;

use crate::context::CtxPair;
use crate::ref_counted_hash_map::{RefCountedHashSet, RemoveResult};
use crate::time::{
    AtomicInstant, Instant, InstantBindingsTypes, InstantContext, TimerBindingsTypes, TimerContext,
    TimerHandler,
};

/// A fake implementation of `Instant` for use in testing.
#[derive(Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct FakeInstant {
    /// A FakeInstant is just an offset from some arbitrary epoch.
    pub offset: Duration,
}

impl crate::inspect::InspectableValue for FakeInstant {
    fn record<I: crate::inspect::Inspector>(&self, name: &str, inspector: &mut I) {
        inspector.record_uint(name, self.offset.as_nanos() as u64)
    }
}

impl FakeInstant {
    /// The maximum value represented by a fake instant.
    pub const LATEST: FakeInstant = FakeInstant { offset: Duration::MAX };

    /// Adds to this fake instant, saturating at [`LATEST`].
    pub fn saturating_add(self, dur: Duration) -> FakeInstant {
        FakeInstant { offset: self.offset.saturating_add(dur) }
    }
}

impl From<Duration> for FakeInstant {
    fn from(offset: Duration) -> FakeInstant {
        FakeInstant { offset }
    }
}

impl Instant for FakeInstant {
    fn checked_duration_since(&self, earlier: FakeInstant) -> Option<Duration> {
        self.offset.checked_sub(earlier.offset)
    }

    fn checked_add(&self, duration: Duration) -> Option<FakeInstant> {
        self.offset.checked_add(duration).map(|offset| FakeInstant { offset })
    }

    fn saturating_add(&self, duration: Duration) -> Self {
        FakeInstant { offset: self.offset.saturating_add(duration) }
    }

    fn checked_sub(&self, duration: Duration) -> Option<FakeInstant> {
        self.offset.checked_sub(duration).map(|offset| FakeInstant { offset })
    }
}

impl ops::Add<Duration> for FakeInstant {
    type Output = FakeInstant;

    fn add(self, dur: Duration) -> FakeInstant {
        FakeInstant { offset: self.offset + dur }
    }
}

impl ops::Sub<FakeInstant> for FakeInstant {
    type Output = Duration;

    fn sub(self, other: FakeInstant) -> Duration {
        self.offset - other.offset
    }
}

impl ops::Sub<Duration> for FakeInstant {
    type Output = FakeInstant;

    fn sub(self, dur: Duration) -> FakeInstant {
        FakeInstant { offset: self.offset - dur }
    }
}

impl Debug for FakeInstant {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.offset)
    }
}

/// A fake implementation of [`AtomicInstant`] for use in testing.
#[derive(Debug)]
pub struct FakeAtomicInstant {
    value: Mutex<FakeInstant>,
}

impl AtomicInstant<FakeInstant> for FakeAtomicInstant {
    fn new(instant: FakeInstant) -> FakeAtomicInstant {
        FakeAtomicInstant { value: Mutex::new(instant) }
    }

    fn load(&self, _ordering: Ordering) -> FakeInstant {
        *self.value.lock()
    }

    fn store(&self, instant: FakeInstant, _ordering: Ordering) {
        *self.value.lock() = instant
    }

    fn store_max(&self, instant: FakeInstant, _ordering: Ordering) {
        let mut guard = self.value.lock();
        if *guard < instant {
            *guard = instant
        }
    }
}

/// A fake [`InstantContext`] which stores the current time as a
/// [`FakeInstant`].
#[derive(Default)]
pub struct FakeInstantCtx {
    /// The fake instant held by this fake context.
    pub time: FakeInstant,
}

impl FakeInstantCtx {
    /// Advance the current time by the given duration.
    pub fn sleep(&mut self, dur: Duration) {
        self.time.offset += dur;
    }

    /// Advance the current time to the given instant.
    ///
    /// Panics if `instant` is not in the future compared to current time.
    pub fn sleep_until(&mut self, instant: FakeInstant) {
        assert!(instant > self.time, "sleep until the past not allowed");
        self.time = instant;
    }
}

impl InstantBindingsTypes for FakeInstantCtx {
    type Instant = FakeInstant;
    type AtomicInstant = FakeAtomicInstant;
}

impl InstantContext for FakeInstantCtx {
    fn now(&self) -> FakeInstant {
        self.time
    }
}

/// Arbitrary data of type `D` attached to a `FakeInstant`.
///
/// `InstantAndData` implements `Ord` and `Eq` to be used in a `BinaryHeap`
/// and ordered by `FakeInstant`.
#[derive(Clone, Debug)]
pub struct InstantAndData<D>(pub FakeInstant, pub D);

impl<D> InstantAndData<D> {
    /// Creates a new `InstantAndData`.
    pub fn new(time: FakeInstant, data: D) -> Self {
        Self(time, data)
    }
}

impl<D> Eq for InstantAndData<D> {}

impl<D> PartialEq for InstantAndData<D> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<D> Ord for InstantAndData<D> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        other.0.cmp(&self.0)
    }
}

impl<D> PartialOrd for InstantAndData<D> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A fake timer vended by [`FakeTimerCtx`].
#[derive(Debug, Clone)]
pub struct FakeTimer<Id> {
    timer_id: usize,
    pub dispatch_id: Id,
}

impl<Id> FakeTimer<Id> {
    /// Gets the unique ID for this timer.
    pub fn timer_id(&self) -> FakeTimerId {
        FakeTimerId(Some(self.timer_id))
    }

    /// Consumes this `FakeTimer` returning the dispatch id and the timer id.
    pub fn into_dispatch_and_timer_id(self) -> (Id, FakeTimerId) {
        let Self { timer_id, dispatch_id } = self;
        (dispatch_id, FakeTimerId(Some(timer_id)))
    }
}

/// The unique timer identifier for [`FakeTimer`].
///
/// The default implementation returns a timer ID that doesn't match any timers.
#[derive(PartialEq, Eq, Copy, Clone, Debug, Default)]
pub struct FakeTimerId(Option<usize>);

/// A fake [`TimerContext`] which stores time as a [`FakeInstantCtx`].
pub struct FakeTimerCtx<Id> {
    /// The instant context within this fake timer context.
    pub instant: FakeInstantCtx,
    /// The timer heap kept by the fake implementation.
    pub timers: BinaryHeap<InstantAndData<FakeTimer<Id>>>,
    /// Used to issue new [`FakeTimer`] ids.
    next_timer_id: usize,
}

impl<Id> Default for FakeTimerCtx<Id> {
    fn default() -> FakeTimerCtx<Id> {
        FakeTimerCtx {
            instant: FakeInstantCtx::default(),
            timers: BinaryHeap::default(),
            next_timer_id: 0,
        }
    }
}

impl<Id: Clone> FakeTimerCtx<Id> {
    /// Get an ordered list of all currently-scheduled timers.
    pub fn timers(&self) -> Vec<(FakeInstant, Id)> {
        self.timers
            .clone()
            .into_sorted_vec()
            .into_iter()
            .map(|InstantAndData(i, FakeTimer { timer_id: _, dispatch_id })| (i, dispatch_id))
            .collect()
    }
}

impl<Id: Debug + Clone + Hash + Eq> FakeTimerCtx<Id> {
    /// Asserts that `self` contains exactly the timers in `timers`.
    ///
    /// # Panics
    ///
    /// Panics if `timers` contains the same ID more than once or if `self`
    /// does not contain exactly the timers in `timers`.
    ///
    /// [`RangeBounds<FakeInstant>`]: core::ops::RangeBounds
    #[track_caller]
    pub fn assert_timers_installed<I: IntoIterator<Item = (Id, FakeInstant)>>(&self, timers: I) {
        self.assert_timers_installed_range(
            timers.into_iter().map(|(id, instant)| (id, instant..=instant)),
        );
    }

    /// Like [`assert_timers_installed`] but receives a range instants to
    /// match.
    ///
    /// Each timer must be present, and its deadline must fall into the
    /// specified range.
    #[track_caller]
    pub fn assert_timers_installed_range<
        R: ops::RangeBounds<FakeInstant> + Debug,
        I: IntoIterator<Item = (Id, R)>,
    >(
        &self,
        timers: I,
    ) {
        self.assert_timers_installed_inner(timers, true);
    }

    /// Asserts that `self` contains at least the timers in `timers`.
    ///
    /// Like [`assert_timers_installed`], but only asserts that `timers` is
    /// a subset of the timers installed; other timers may be installed in
    /// addition to those in `timers`.
    #[track_caller]
    pub fn assert_some_timers_installed<I: IntoIterator<Item = (Id, FakeInstant)>>(
        &self,
        timers: I,
    ) {
        self.assert_some_timers_installed_range(
            timers.into_iter().map(|(id, instant)| (id, instant..=instant)),
        );
    }

    /// Like [`assert_some_timers_installed`] but receives instant ranges
    /// to match like [`assert_timers_installed_range`].
    #[track_caller]
    pub fn assert_some_timers_installed_range<
        R: ops::RangeBounds<FakeInstant> + Debug,
        I: IntoIterator<Item = (Id, R)>,
    >(
        &self,
        timers: I,
    ) {
        self.assert_timers_installed_inner(timers, false);
    }

    /// Asserts that no timers are installed.
    ///
    /// # Panics
    ///
    /// Panics if any timers are installed.
    #[track_caller]
    pub fn assert_no_timers_installed(&self) {
        self.assert_timers_installed([]);
    }

    #[track_caller]
    fn assert_timers_installed_inner<
        R: ops::RangeBounds<FakeInstant> + Debug,
        I: IntoIterator<Item = (Id, R)>,
    >(
        &self,
        timers: I,
        exact: bool,
    ) {
        let mut timers = timers.into_iter().fold(HashMap::new(), |mut timers, (id, range)| {
            assert_matches!(timers.insert(id, range), None);
            timers
        });

        enum Error<Id, R: ops::RangeBounds<FakeInstant>> {
            ExpectedButMissing { id: Id, range: R },
            UnexpectedButPresent { id: Id, instant: FakeInstant },
            UnexpectedInstant { id: Id, range: R, instant: FakeInstant },
        }

        let mut errors = Vec::new();

        // Make sure that all installed timers were expected (present in
        // `timers`).
        for InstantAndData(instant, FakeTimer { timer_id: _, dispatch_id: id }) in
            self.timers.iter().cloned()
        {
            match timers.remove(&id) {
                None => {
                    if exact {
                        errors.push(Error::UnexpectedButPresent { id, instant })
                    }
                }
                Some(range) => {
                    if !range.contains(&instant) {
                        errors.push(Error::UnexpectedInstant { id, range, instant })
                    }
                }
            }
        }

        // Make sure that all expected timers were already found in
        // `self.timers` (and removed from `timers`).
        errors.extend(timers.drain().map(|(id, range)| Error::ExpectedButMissing { id, range }));

        if errors.len() > 0 {
            let mut s = String::from("Unexpected timer contents:");
            for err in errors {
                s += &match err {
                    Error::ExpectedButMissing { id, range } => {
                        format!("\n\tMissing timer {:?} with deadline {:?}", id, range)
                    }
                    Error::UnexpectedButPresent { id, instant } => {
                        format!("\n\tUnexpected timer {:?} with deadline {:?}", id, instant)
                    }
                    Error::UnexpectedInstant { id, range, instant } => format!(
                        "\n\tTimer {:?} has unexpected deadline {:?} (wanted {:?})",
                        id, instant, range
                    ),
                };
            }
            panic!("{}", s);
        }
    }
}

impl<Id> FakeTimerCtx<Id> {
    /// Pops the next timer to fire, returning its Id and advances the current
    /// time.
    pub fn pop_next_timer_and_advance_time(&mut self) -> Option<Id> {
        let InstantAndData(instant, timer) = self.timers.pop()?;
        assert!(instant >= self.instant.time);
        self.instant.time = instant;
        Some(timer.dispatch_id)
    }
}

impl<Id: PartialEq> FakeTimerCtx<Id> {
    fn cancel_timer_inner(&mut self, timer: &FakeTimer<Id>) -> Option<FakeInstant> {
        let mut r: Option<FakeInstant> = None;
        // NB: Cancelling timers can be made faster than this if we keep two
        // data structures and require that `Id: Hash`.
        self.timers.retain(|InstantAndData(instant, FakeTimer { timer_id, dispatch_id: _ })| {
            if timer.timer_id == *timer_id {
                r = Some(*instant);
                false
            } else {
                true
            }
        });
        r
    }
}

impl<Id> InstantBindingsTypes for FakeTimerCtx<Id> {
    type Instant = FakeInstant;
    type AtomicInstant = FakeAtomicInstant;
}

impl<Id> InstantContext for FakeTimerCtx<Id> {
    fn now(&self) -> FakeInstant {
        self.instant.now()
    }
}

impl<Id: Debug + Clone + Send + Sync> TimerBindingsTypes for FakeTimerCtx<Id> {
    type Timer = FakeTimer<Id>;
    type DispatchId = Id;
    type UniqueTimerId = FakeTimerId;
}

impl<Id: PartialEq + Debug + Clone + Send + Sync> TimerContext for FakeTimerCtx<Id> {
    fn new_timer(&mut self, dispatch_id: Self::DispatchId) -> Self::Timer {
        let timer_id = self.next_timer_id;
        self.next_timer_id += 1;
        FakeTimer { timer_id, dispatch_id }
    }

    fn schedule_timer_instant(
        &mut self,
        time: Self::Instant,
        timer: &mut Self::Timer,
    ) -> Option<Self::Instant> {
        let ret = self.cancel_timer_inner(timer);
        self.timers.push(InstantAndData::new(time, timer.clone()));
        ret
    }

    fn cancel_timer(&mut self, timer: &mut Self::Timer) -> Option<Self::Instant> {
        self.cancel_timer_inner(timer)
    }

    fn scheduled_instant(&self, timer: &mut Self::Timer) -> Option<Self::Instant> {
        self.timers.iter().find_map(
            |InstantAndData(instant, FakeTimer { timer_id, dispatch_id: _ })| {
                (timer.timer_id == *timer_id).then_some(*instant)
            },
        )
    }

    fn unique_timer_id(&self, timer: &Self::Timer) -> Self::UniqueTimerId {
        timer.timer_id()
    }
}

/// A trait abstracting access to a [`FakeTimerCtx`] instance.
pub trait WithFakeTimerContext<TimerId> {
    /// Calls the callback with a borrow of `FakeTimerCtx`.
    fn with_fake_timer_ctx<O, F: FnOnce(&FakeTimerCtx<TimerId>) -> O>(&self, f: F) -> O;

    /// Calls the callback with a mutable borrow of `FakeTimerCtx`.
    fn with_fake_timer_ctx_mut<O, F: FnOnce(&mut FakeTimerCtx<TimerId>) -> O>(&mut self, f: F)
    -> O;
}

impl<TimerId> WithFakeTimerContext<TimerId> for FakeTimerCtx<TimerId> {
    fn with_fake_timer_ctx<O, F: FnOnce(&FakeTimerCtx<TimerId>) -> O>(&self, f: F) -> O {
        f(self)
    }

    fn with_fake_timer_ctx_mut<O, F: FnOnce(&mut FakeTimerCtx<TimerId>) -> O>(
        &mut self,
        f: F,
    ) -> O {
        f(self)
    }
}

impl<TimerId, CC, BC> WithFakeTimerContext<TimerId> for CtxPair<CC, BC>
where
    BC: WithFakeTimerContext<TimerId>,
{
    fn with_fake_timer_ctx<O, F: FnOnce(&FakeTimerCtx<TimerId>) -> O>(&self, f: F) -> O {
        self.bindings_ctx.with_fake_timer_ctx(f)
    }

    fn with_fake_timer_ctx_mut<O, F: FnOnce(&mut FakeTimerCtx<TimerId>) -> O>(
        &mut self,
        f: F,
    ) -> O {
        self.bindings_ctx.with_fake_timer_ctx_mut(f)
    }
}

/// Adds methods for interacting with [`FakeTimerCtx`] and its wrappers.
pub trait FakeTimerCtxExt<Id>: Sized + TimerBindingsTypes {
    /// Triggers the next timer, if any, by using the provided `handler`.
    ///
    /// `trigger_next_timer` triggers the next timer, if any, advances the
    /// internal clock to the timer's scheduled time, and returns its ID.
    fn trigger_next_timer<H: TimerHandler<Self, Id>>(&mut self, handler: &mut H) -> Option<Id>;

    /// Skips the current time forward until `instant`, triggering all timers
    /// until then, inclusive, by calling `f` on them.
    ///
    /// Returns the timers which were triggered.
    ///
    /// # Panics
    ///
    /// Panics if `instant` is in the past.
    fn trigger_timers_until_instant<H: TimerHandler<Self, Id>>(
        &mut self,
        instant: FakeInstant,
        handler: &mut H,
    ) -> Vec<Id>;

    /// Skips the current time forward by `duration`, triggering all timers
    /// until then, inclusive, by passing them to the `handler`.
    ///
    /// Returns the timers which were triggered.
    fn trigger_timers_for<H: TimerHandler<Self, Id>>(
        &mut self,
        duration: Duration,
        handler: &mut H,
    ) -> Vec<Id>;

    /// Triggers timers and expects them to be the given timers.
    ///
    /// The number of timers to be triggered is taken to be the number of timers
    /// produced by `timers`. Timers may be triggered in any order.
    ///
    /// # Panics
    ///
    /// Panics under the following conditions:
    /// - Fewer timers could be triggered than expected
    /// - Timers were triggered that were not expected
    /// - Timers that were expected were not triggered
    #[track_caller]
    fn trigger_timers_and_expect_unordered<I: IntoIterator<Item = Id>, H: TimerHandler<Self, Id>>(
        &mut self,
        timers: I,
        handler: &mut H,
    ) where
        Id: Debug + Hash + Eq;

    /// Triggers timers until `instant` and expects them to be the given timers.
    ///
    /// Like `trigger_timers_and_expect_unordered`, except that timers will only
    /// be triggered until `instant` (inclusive).
    fn trigger_timers_until_and_expect_unordered<
        I: IntoIterator<Item = Id>,
        H: TimerHandler<Self, Id>,
    >(
        &mut self,
        instant: FakeInstant,
        timers: I,
        handler: &mut H,
    ) where
        Id: Debug + Hash + Eq;

    /// Triggers timers for `duration` and expects them to be the given timers.
    ///
    /// Like `trigger_timers_and_expect_unordered`, except that timers will only
    /// be triggered for `duration` (inclusive).
    fn trigger_timers_for_and_expect<I: IntoIterator<Item = Id>, H: TimerHandler<Self, Id>>(
        &mut self,
        duration: Duration,
        timers: I,
        handler: &mut H,
    ) where
        Id: Debug + Hash + Eq;
}

// TODO(https://fxbug.dev/42081080): hold lock on `FakeTimerCtx` across entire
// method to avoid potential race conditions.
impl<Id: Clone, Ctx: WithFakeTimerContext<Id> + TimerBindingsTypes<UniqueTimerId = FakeTimerId>>
    FakeTimerCtxExt<Id> for Ctx
{
    /// Triggers the next timer, if any, by calling `f` on it.
    ///
    /// `trigger_next_timer` triggers the next timer, if any, advances the
    /// internal clock to the timer's scheduled time, and returns its ID.
    fn trigger_next_timer<H: TimerHandler<Self, Id>>(&mut self, handler: &mut H) -> Option<Id> {
        self.with_fake_timer_ctx_mut(|timers| {
            timers.timers.pop().map(|InstantAndData(t, id)| {
                timers.instant.time = t;
                id
            })
        })
        .map(|fake_timer| {
            let (dispatch_id, timer_id) = fake_timer.into_dispatch_and_timer_id();
            handler.handle_timer(self, dispatch_id.clone(), timer_id);
            dispatch_id
        })
    }

    /// Skips the current time forward until `instant`, triggering all timers
    /// until then, inclusive, by giving them to `handler`.
    ///
    /// Returns the timers which were triggered.
    ///
    /// # Panics
    ///
    /// Panics if `instant` is in the past.
    fn trigger_timers_until_instant<H: TimerHandler<Self, Id>>(
        &mut self,
        instant: FakeInstant,
        handler: &mut H,
    ) -> Vec<Id> {
        assert!(instant >= self.with_fake_timer_ctx(|ctx| ctx.now()));
        let mut timers = Vec::new();

        while self.with_fake_timer_ctx_mut(|ctx| {
            ctx.timers.peek().map(|InstantAndData(i, _id)| i <= &instant).unwrap_or(false)
        }) {
            timers.push(self.trigger_next_timer(handler).unwrap())
        }

        self.with_fake_timer_ctx_mut(|ctx| {
            assert!(ctx.now() <= instant);
            ctx.instant.time = instant;
        });

        timers
    }

    /// Skips the current time forward by `duration`, triggering all timers
    /// until then, inclusive, by calling `f` on them.
    ///
    /// Returns the timers which were triggered.
    fn trigger_timers_for<H: TimerHandler<Self, Id>>(
        &mut self,
        duration: Duration,
        handler: &mut H,
    ) -> Vec<Id> {
        let instant = self.with_fake_timer_ctx(|ctx| ctx.now().saturating_add(duration));
        // We know the call to `self.trigger_timers_until_instant` will not
        // panic because we provide an instant that is greater than or equal
        // to the current time.
        self.trigger_timers_until_instant(instant, handler)
    }

    /// Triggers timers and expects them to be the given timers.
    ///
    /// The number of timers to be triggered is taken to be the number of
    /// timers produced by `timers`. Timers may be triggered in any order.
    ///
    /// # Panics
    ///
    /// Panics under the following conditions:
    /// - Fewer timers could be triggered than expected
    /// - Timers were triggered that were not expected
    /// - Timers that were expected were not triggered
    #[track_caller]
    fn trigger_timers_and_expect_unordered<I: IntoIterator<Item = Id>, H: TimerHandler<Self, Id>>(
        &mut self,
        timers: I,
        handler: &mut H,
    ) where
        Id: Debug + Hash + Eq,
    {
        let mut timers = RefCountedHashSet::from_iter(timers);

        for _ in 0..timers.len() {
            let id = self.trigger_next_timer(handler).expect("ran out of timers to trigger");
            match timers.remove(id.clone()) {
                RemoveResult::Removed(()) | RemoveResult::StillPresent => {}
                RemoveResult::NotPresent => panic!("triggered unexpected timer: {:?}", id),
            }
        }

        if timers.len() > 0 {
            let mut s = String::from("Expected timers did not trigger:");
            for (id, count) in timers.iter_counts() {
                s += &format!("\n\t{count}x {id:?}");
            }
            panic!("{}", s);
        }
    }

    /// Triggers timers until `instant` and expects them to be the given
    /// timers.
    ///
    /// Like `trigger_timers_and_expect_unordered`, except that timers will
    /// only be triggered until `instant` (inclusive).
    fn trigger_timers_until_and_expect_unordered<
        I: IntoIterator<Item = Id>,
        H: TimerHandler<Self, Id>,
    >(
        &mut self,
        instant: FakeInstant,
        timers: I,
        handler: &mut H,
    ) where
        Id: Debug + Hash + Eq,
    {
        let mut timers = RefCountedHashSet::from_iter(timers);

        let triggered_timers = self.trigger_timers_until_instant(instant, handler);

        for id in triggered_timers {
            match timers.remove(id.clone()) {
                RemoveResult::Removed(()) | RemoveResult::StillPresent => {}
                RemoveResult::NotPresent => panic!("triggered unexpected timer: {:?}", id),
            }
        }

        if timers.len() > 0 {
            let mut s = String::from("Expected timers did not trigger:");
            for (id, count) in timers.iter_counts() {
                s += &format!("\n\t{count}x {id:?}");
            }
            panic!("{}", s);
        }
    }

    /// Triggers timers for `duration` and expects them to be the given
    /// timers.
    ///
    /// Like `trigger_timers_and_expect_unordered`, except that timers will
    /// only be triggered for `duration` (inclusive).
    fn trigger_timers_for_and_expect<I: IntoIterator<Item = Id>, H: TimerHandler<Self, Id>>(
        &mut self,
        duration: Duration,
        timers: I,
        handler: &mut H,
    ) where
        Id: Debug + Hash + Eq,
    {
        let instant = self.with_fake_timer_ctx(|ctx| ctx.now().saturating_add(duration));
        self.trigger_timers_until_and_expect_unordered(instant, timers, handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::HandleableTimer;

    use alloc::vec;

    const ONE_SEC: Duration = Duration::from_secs(1);
    const ONE_SEC_INSTANT: FakeInstant = FakeInstant { offset: ONE_SEC };

    #[derive(Debug, Eq, PartialEq, Clone, Hash)]
    struct TimerId(usize);
    #[derive(Default)]
    struct CoreCtx(Vec<(TimerId, FakeInstant)>);

    impl CoreCtx {
        fn take(&mut self) -> Vec<(TimerId, FakeInstant)> {
            core::mem::take(&mut self.0)
        }
    }

    impl HandleableTimer<CoreCtx, FakeTimerCtx<Self>> for TimerId {
        fn handle(
            self,
            CoreCtx(expired): &mut CoreCtx,
            bindings_ctx: &mut FakeTimerCtx<Self>,
            _: FakeTimerId,
        ) {
            expired.push((self, bindings_ctx.now()))
        }
    }

    #[test]
    fn instant_and_data() {
        // Verify implementation of InstantAndData to be used as a complex
        // type in a BinaryHeap.
        let mut heap = BinaryHeap::<InstantAndData<usize>>::new();
        let now = FakeInstant::default();

        fn new_data(time: FakeInstant, id: usize) -> InstantAndData<usize> {
            InstantAndData::new(time, id)
        }

        heap.push(new_data(now + Duration::from_secs(1), 1));
        heap.push(new_data(now + Duration::from_secs(2), 2));

        // Earlier timer is popped first.
        assert_eq!(heap.pop().unwrap().1, 1);
        assert_eq!(heap.pop().unwrap().1, 2);
        assert_eq!(heap.pop(), None);

        heap.push(new_data(now + Duration::from_secs(1), 1));
        heap.push(new_data(now + Duration::from_secs(1), 1));

        // Can pop twice with identical data.
        assert_eq!(heap.pop().unwrap().1, 1);
        assert_eq!(heap.pop().unwrap().1, 1);
        assert_eq!(heap.pop(), None);
    }

    #[test]
    fn fake_timer_context() {
        let mut core_ctx = CoreCtx::default();
        let mut bindings_ctx = FakeTimerCtx::<TimerId>::default();

        // When no timers are installed, `trigger_next_timer` should return
        // `false`.
        assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), None);
        assert_eq!(core_ctx.take(), vec![]);

        let mut timer0 = bindings_ctx.new_timer(TimerId(0));
        let mut timer1 = bindings_ctx.new_timer(TimerId(1));
        let mut timer2 = bindings_ctx.new_timer(TimerId(2));

        // No timer with id `0` exists yet.
        assert_eq!(bindings_ctx.scheduled_instant(&mut timer0), None);

        assert_eq!(bindings_ctx.schedule_timer(ONE_SEC, &mut timer0), None);

        // Timer with id `0` scheduled to execute at `ONE_SEC_INSTANT`.
        assert_eq!(bindings_ctx.scheduled_instant(&mut timer0).unwrap(), ONE_SEC_INSTANT);

        assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(TimerId(0)));
        assert_eq!(core_ctx.take(), vec![(TimerId(0), ONE_SEC_INSTANT)]);

        // After the timer fires, it should not still be scheduled at some
        // instant.
        assert_eq!(bindings_ctx.scheduled_instant(&mut timer0), None);

        // The time should have been advanced.
        assert_eq!(bindings_ctx.now(), ONE_SEC_INSTANT);

        // Once it's been triggered, it should be canceled and not
        // triggerable again.
        assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), None);
        assert_eq!(core_ctx.take(), vec![]);

        // Unwind back time.
        bindings_ctx.instant.time = Default::default();

        // If we schedule a timer but then cancel it, it shouldn't fire.
        assert_eq!(bindings_ctx.schedule_timer(ONE_SEC, &mut timer0), None);
        assert_eq!(bindings_ctx.cancel_timer(&mut timer0), Some(ONE_SEC_INSTANT));
        assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), None);
        assert_eq!(core_ctx.take(), vec![]);

        // If we schedule a timer but then schedule the same ID again, the
        // second timer should overwrite the first one.
        assert_eq!(bindings_ctx.schedule_timer(Duration::from_secs(0), &mut timer0), None);
        assert_eq!(
            bindings_ctx.schedule_timer(ONE_SEC, &mut timer0),
            Some(Duration::from_secs(0).into())
        );
        assert_eq!(bindings_ctx.cancel_timer(&mut timer0), Some(ONE_SEC_INSTANT));

        // If we schedule three timers and then run `trigger_timers_until`
        // with the appropriate value, only two of them should fire.
        assert_eq!(bindings_ctx.schedule_timer(Duration::from_secs(0), &mut timer0), None);
        assert_eq!(bindings_ctx.schedule_timer(Duration::from_secs(1), &mut timer1), None);
        assert_eq!(bindings_ctx.schedule_timer(Duration::from_secs(2), &mut timer2), None);
        assert_eq!(
            bindings_ctx.trigger_timers_until_instant(ONE_SEC_INSTANT, &mut core_ctx),
            vec![TimerId(0), TimerId(1)],
        );

        // The first two timers should have fired.
        assert_eq!(
            core_ctx.take(),
            vec![
                (TimerId(0), FakeInstant::from(Duration::from_secs(0))),
                (TimerId(1), ONE_SEC_INSTANT)
            ]
        );

        // They should be canceled now.
        assert_eq!(bindings_ctx.cancel_timer(&mut timer0), None);
        assert_eq!(bindings_ctx.cancel_timer(&mut timer1), None);

        // The clock should have been updated.
        assert_eq!(bindings_ctx.now(), ONE_SEC_INSTANT);

        // The last timer should not have fired.
        assert_eq!(
            bindings_ctx.cancel_timer(&mut timer2),
            Some(FakeInstant::from(Duration::from_secs(2)))
        );
    }

    #[test]
    fn trigger_timers_until_and_expect_unordered() {
        // If the requested instant does not coincide with a timer trigger
        // point, the time should still be advanced.
        let mut core_ctx = CoreCtx::default();
        let mut bindings_ctx = FakeTimerCtx::default();
        let mut timer0 = bindings_ctx.new_timer(TimerId(0));
        let mut timer1 = bindings_ctx.new_timer(TimerId(1));
        assert_eq!(bindings_ctx.schedule_timer(Duration::from_secs(0), &mut timer0), None);
        assert_eq!(bindings_ctx.schedule_timer(Duration::from_secs(2), &mut timer1), None);
        bindings_ctx.trigger_timers_until_and_expect_unordered(
            ONE_SEC_INSTANT,
            vec![TimerId(0)],
            &mut core_ctx,
        );
        assert_eq!(bindings_ctx.now(), ONE_SEC_INSTANT);
    }
}
