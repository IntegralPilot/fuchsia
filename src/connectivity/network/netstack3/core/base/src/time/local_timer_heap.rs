// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A local timer heap for use in netstack3 core.

use alloc::collections::{BinaryHeap, binary_heap};
use core::hash::Hash;
use core::time::Duration;

use netstack3_hashmap::{HashMap, hash_map};

use crate::{CoreTimerContext, Instant, InstantBindingsTypes, TimerBindingsTypes, TimerContext};

/// A local timer heap that keeps timers for core modules.
///
/// `LocalTimerHeap` manages its wakeups through a [`TimerContext`].
///
/// `K` is the key that timers are keyed on. `V` is optional sidecar data to be
/// kept with each timer.
///
/// Note that to provide fast timer deletion, `LocalTimerHeap` requires `K` to
/// be `Clone` and the implementation assumes that the clone is cheap.
#[derive(Debug)]
pub struct LocalTimerHeap<K, V, BT: TimerBindingsTypes + InstantBindingsTypes> {
    next_wakeup: BT::Timer,
    heap: KeyedHeap<K, V, BT::Instant>,
}

impl<K, V, BC> LocalTimerHeap<K, V, BC>
where
    K: Hash + Eq + Clone,
    BC: TimerContext,
{
    /// Creates a new `LocalTimerHeap` with wakeup dispatch ID `dispatch_id`.
    pub fn new(bindings_ctx: &mut BC, dispatch_id: BC::DispatchId) -> Self {
        let next_wakeup = bindings_ctx.new_timer(dispatch_id);
        Self { next_wakeup, heap: KeyedHeap::new() }
    }

    /// Like [`new`] but uses `CC` to covert the `dispatch_id` to match the type required by `BC`.
    pub fn new_with_context<D, CC: CoreTimerContext<D, BC>>(
        bindings_ctx: &mut BC,
        dispatch_id: D,
    ) -> Self {
        Self::new(bindings_ctx, CC::convert_timer(dispatch_id))
    }

    /// Schedules `timer` with `value` at or after `at`.
    ///
    /// If `timer` was already scheduled, returns the previously scheduled
    /// instant and the associated value.
    pub fn schedule_instant(
        &mut self,
        bindings_ctx: &mut BC,
        timer: K,
        value: V,
        at: BC::Instant,
    ) -> Option<(BC::Instant, V)> {
        let (prev_value, dirty) = self.heap.schedule(timer, value, at);
        if dirty {
            self.heal_and_reschedule(bindings_ctx);
        }
        prev_value
    }

    /// Like [`schedule_instant`] but does the instant math from current time.
    ///
    /// # Panics
    ///
    /// Panics if the current `BC::Instant` cannot be represented by adding
    /// duration `after`.
    pub fn schedule_after(
        &mut self,
        bindings_ctx: &mut BC,
        timer: K,
        value: V,
        after: Duration,
    ) -> Option<(BC::Instant, V)> {
        let time = bindings_ctx.now().checked_add(after).unwrap();
        self.schedule_instant(bindings_ctx, timer, value, time)
    }

    /// Pops an expired timer from the heap, if any.
    pub fn pop(&mut self, bindings_ctx: &mut BC) -> Option<(K, V)> {
        let Self { next_wakeup: _, heap } = self;
        let (popped, dirty) = heap.pop_if(|t| t <= bindings_ctx.now());
        if dirty {
            self.heal_and_reschedule(bindings_ctx);
        }
        popped
    }

    /// Returns the scheduled instant and associated value for `timer`, if it's
    /// scheduled.
    pub fn get(&self, timer: &K) -> Option<(BC::Instant, &V)> {
        self.heap.map.get(timer).map(|MapEntry { time, value }| (*time, value))
    }

    /// Cancels `timer`, returning the scheduled instant and associated value if
    /// any.
    pub fn cancel(&mut self, bindings_ctx: &mut BC, timer: &K) -> Option<(BC::Instant, V)> {
        let (scheduled, dirty) = self.heap.cancel(timer);
        if dirty {
            self.heal_and_reschedule(bindings_ctx);
        }
        scheduled
    }

    /// Gets an iterator over the installed timers.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V, &BC::Instant)> {
        self.heap.map.iter().map(|(k, MapEntry { time, value })| (k, value, time))
    }

    fn heal_and_reschedule(&mut self, bindings_ctx: &mut BC) {
        let Self { next_wakeup, heap } = self;
        let mut new_top = None;
        let _ = heap.pop_if(|t| {
            // Extract the next time to fire, but don't pop it from the heap.
            // This is equivalent to peeking, but it also "heals" the keyed heap
            // by getting rid of stale entries.
            new_top = Some(t);
            false
        });
        let _: Option<BC::Instant> = match new_top {
            Some(time) => bindings_ctx.schedule_timer_instant(time, next_wakeup),
            None => bindings_ctx.cancel_timer(next_wakeup),
        };
    }

    /// Removes all timers.
    pub fn clear(&mut self, bindings_ctx: &mut BC) {
        let Self { next_wakeup, heap } = self;
        heap.clear();
        let _: Option<BC::Instant> = bindings_ctx.cancel_timer(next_wakeup);
    }

    /// Returns true if there are no timers installed.
    pub fn is_empty(&self) -> bool {
        self.heap.map.is_empty()
    }
}

/// A timer heap that is keyed on `K`.
///
/// This type is used to support [`LocalTimerHeap`].
#[derive(Debug)]
struct KeyedHeap<K, V, T> {
    // Implementation note: The map is the source of truth for the desired
    // firing time for a timer `K`. The heap has a copy of the scheduled time
    // that is *always* compared to the map before firing the timer.
    //
    // That allows timers to be rescheduled to a later time during `pop`, which
    // reduces memory utilization for the heap by avoiding the stale entry.
    map: HashMap<K, MapEntry<T, V>>,
    heap: BinaryHeap<HeapEntry<T, K>>,
}

impl<K: Hash + Eq + Clone, V, T: Instant> KeyedHeap<K, V, T> {
    fn new() -> Self {
        Self { map: HashMap::new(), heap: BinaryHeap::new() }
    }

    /// Schedules `key` with associated `value` at time `at`.
    ///
    /// Returns the previously associated value and firing time for `key` +
    /// a boolean indicating whether the top of the heap (i.e. the next timer to
    /// fire) changed with this operation.
    fn schedule(&mut self, key: K, value: V, at: T) -> (Option<(T, V)>, bool) {
        let Self { map, heap } = self;
        // The top of the heap is changed if any of the following is true:
        // - There were previously no entries in the heap.
        // - The current top of the heap is the key being changed here.
        // - The scheduled value is earlier than the current top of the heap.
        let dirty = heap
            .peek()
            .map(|HeapEntry { time, key: top_key }| top_key == &key || at < *time)
            .unwrap_or(true);
        let (heap_entry, prev) = match map.entry(key) {
            hash_map::Entry::Occupied(mut o) => {
                let MapEntry { time, value } = o.insert(MapEntry { time: at, value });
                // Only create a new entry if the already scheduled time is
                // later than the new value.
                let heap_entry = (at < time).then(|| HeapEntry { time: at, key: o.key().clone() });
                (heap_entry, Some((time, value)))
            }
            hash_map::Entry::Vacant(v) => {
                let heap_entry = Some(HeapEntry { time: at, key: v.key().clone() });
                let _: &mut MapEntry<_, _> = v.insert(MapEntry { time: at, value });
                (heap_entry, None)
            }
        };
        if let Some(heap_entry) = heap_entry {
            heap.push(heap_entry);
        }
        (prev, dirty)
    }

    /// Cancels the timer with `key`.
    ///
    /// Returns the scheduled instant and value for `key` if it was scheduled +
    /// a boolean indicating whether the top of the heap (i.e. the next timer to
    /// fire) changed with this operation.
    fn cancel(&mut self, key: &K) -> (Option<(T, V)>, bool) {
        let Self { heap, map } = self;
        // The front of the heap will be changed if we're cancelling the top.
        let was_front = heap.peek().is_some_and(|HeapEntry { time: _, key: top }| key == top);
        let prev = map.remove(key).map(|MapEntry { time, value }| (time, value));
        (prev, was_front)
    }

    /// Heals the heap of stale entries, popping the first valid entry if `f`
    /// returns `true``.
    ///
    /// Returns the popped value if one is found *and* `f` returns true + a
    /// boolean that is `true` iff the internal heap has changed.
    ///
    /// NB: This API is a bit wonky, but unfortunately we can't seem to be able
    /// to express a type that would be equivalent to `BinaryHeap`'s `PeekMut`.
    /// This is the next best thing.
    fn pop_if<F: FnOnce(T) -> bool>(&mut self, f: F) -> (Option<(K, V)>, bool) {
        let mut changed_heap = false;
        let popped = loop {
            let Self { heap, map } = self;
            let Some(peek_mut) = heap.peek_mut() else {
                break None;
            };
            let HeapEntry { time: heap_time, key } = &*peek_mut;
            // Always check the map state for the given key, since it's the
            // source of truth for desired firing time.

            // NB: We assume here that the key is cheaply cloned and that
            // cloning it is faster than possibly hashing it more than once.
            match map.entry(key.clone()) {
                hash_map::Entry::Vacant(_) => {
                    // Timer has been canceled. Pop and continue looking.
                    let _: HeapEntry<_, _> = binary_heap::PeekMut::pop(peek_mut);
                    changed_heap = true;
                }
                hash_map::Entry::Occupied(map_entry) => {
                    let MapEntry { time: scheduled_for, value: _ } = map_entry.get();

                    match heap_time.cmp(scheduled_for) {
                        core::cmp::Ordering::Equal => {
                            // Map and heap agree on firing time, this is the top of
                            // the heap.
                            break f(*scheduled_for).then(|| {
                                let HeapEntry { time: _, key } =
                                    binary_heap::PeekMut::pop(peek_mut);
                                changed_heap = true;
                                let MapEntry { time: _, value } = map_entry.remove();
                                (key, value)
                            });
                        }
                        core::cmp::Ordering::Less => {
                            // When rescheduling a timer, we only touch the heap
                            // if rescheduling to an earlier time. In this case
                            // the map is telling us this is scheduled for
                            // later, so we must put it back in the heap.
                            let HeapEntry { time: _, key } = binary_heap::PeekMut::pop(peek_mut);
                            heap.push(HeapEntry { time: *scheduled_for, key });
                            changed_heap = true;
                        }
                        core::cmp::Ordering::Greater => {
                            // Heap time greater than scheduled time is
                            // effectively unobservable because any earlier
                            // entry would've been popped from the heap already
                            // and thus the entry would be missing from the map.
                            // Even a catastrophic cancel => reschedule cycle
                            // can't make us observe this branch given the heap
                            // ordering properties.
                            unreachable!(
                                "observed heap time: {:?} later than the scheduled time {:?}",
                                heap_time, scheduled_for
                            );
                        }
                    }
                }
            }
        };
        (popped, changed_heap)
    }

    fn clear(&mut self) {
        let Self { map, heap } = self;
        map.clear();
        heap.clear();
    }
}

/// The entry kept in [`LocalTimerHeap`]'s internal hash map.
#[derive(Debug, Eq, PartialEq)]
struct MapEntry<T, V> {
    time: T,
    value: V,
}

/// A reusable struct to place a value and a timestamp in a [`BinaryHeap`].
///
/// Its `Ord` implementation is tuned to make [`BinaryHeap`] a min heap.
#[derive(Debug)]
struct HeapEntry<T, K> {
    time: T,
    key: K,
}

// Boilerplate to implement a heap entry.
impl<T: Instant, K> PartialEq for HeapEntry<T, K> {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time
    }
}

impl<T: Instant, K> Eq for HeapEntry<T, K> {}

impl<T: Instant, K> Ord for HeapEntry<T, K> {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        // Note that we flip the argument order here to make the `BinaryHeap` a
        // min heap.
        Ord::cmp(&other.time, &self.time)
    }
}

impl<T: Instant, K> PartialOrd for HeapEntry<T, K> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(Ord::cmp(self, other))
    }
}

#[cfg(any(test, feature = "testutils"))]
mod testutil {
    use core::fmt::Debug;
    use core::ops::RangeBounds;

    use super::*;

    impl<K, V, BC> LocalTimerHeap<K, V, BC>
    where
        K: Hash + Eq + Clone + Debug,
        V: Debug + Eq + PartialEq,
        BC: TimerContext,
    {
        /// Asserts installed timers with an iterator of `(key, value, instant)`
        /// tuples.
        #[track_caller]
        pub fn assert_timers(&self, timers: impl IntoIterator<Item = (K, V, BC::Instant)>) {
            let map = timers
                .into_iter()
                .map(|(k, value, time)| (k, MapEntry { value, time }))
                .collect::<HashMap<_, _>>();
            assert_eq!(&self.heap.map, &map);
        }

        /// Like [`LocalTimerHeap::assert_timers`], but asserts based on a
        /// duration after `bindings_ctx.now()`.
        #[track_caller]
        pub fn assert_timers_after(
            &self,
            bindings_ctx: &mut BC,
            timers: impl IntoIterator<Item = (K, V, Duration)>,
        ) {
            let now = bindings_ctx.now();
            self.assert_timers(timers.into_iter().map(|(k, v, d)| (k, v, now.panicking_add(d))))
        }

        /// Assets that the next time to fire has `key` and `value`.
        #[track_caller]
        pub fn assert_top(&mut self, key: &K, value: &V) {
            // NB: We can't know that the top of the heap holds a valid entry,
            // so we need to do the slow thing and look in the map for this
            // assertion.
            let top = self
                .heap
                .map
                .iter()
                .min_by_key(|(_key, MapEntry { time, .. })| time)
                .map(|(key, MapEntry { time: _, value })| (key, value));
            assert_eq!(top, Some((key, value)));
        }

        /// Asserts that the given timer is installed with an instant at the
        /// provided range.
        #[track_caller]
        pub fn assert_range<
            'a,
            R: RangeBounds<BC::Instant> + Debug,
            I: IntoIterator<Item = (&'a K, R)>,
        >(
            &'a self,
            expect: I,
        ) {
            for (timer, range) in expect {
                let time = self
                    .get(timer)
                    .map(|(t, _)| t)
                    .unwrap_or_else(|| panic!("timer {timer:?} not present"));
                assert!(range.contains(&time), "timer {timer:?} is at {time:?} not in {range:?}");
            }
        }

        /// Asserts that the given timer is installed with an instant at the
        /// provided range, returning its information.
        #[track_caller]
        pub fn assert_range_single<'a, R: RangeBounds<BC::Instant> + Debug>(
            &'a self,
            timer: &K,
            range: R,
        ) -> (BC::Instant, &'a V) {
            let (time, value) =
                self.get(timer).unwrap_or_else(|| panic!("timer {timer:?} not present"));
            assert!(range.contains(&time), "timer {timer:?} is at {time:?} not in {range:?}");
            (time, value)
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::convert::Infallible as Never;

    use crate::InstantContext;
    use crate::testutil::{FakeAtomicInstant, FakeInstant, FakeInstantCtx};

    use super::*;

    #[derive(Default)]
    struct FakeTimerCtx {
        instant: FakeInstantCtx,
    }

    impl InstantBindingsTypes for FakeTimerCtx {
        type Instant = FakeInstant;
        type AtomicInstant = FakeAtomicInstant;
    }

    impl InstantContext for FakeTimerCtx {
        fn now(&self) -> Self::Instant {
            self.instant.now()
        }
    }

    impl TimerBindingsTypes for FakeTimerCtx {
        type Timer = FakeTimer;
        type DispatchId = ();
        type UniqueTimerId = Never;
    }

    impl TimerContext for FakeTimerCtx {
        fn new_timer(&mut self, (): Self::DispatchId) -> Self::Timer {
            FakeTimer::default()
        }

        fn schedule_timer_instant(
            &mut self,
            time: Self::Instant,
            timer: &mut Self::Timer,
        ) -> Option<Self::Instant> {
            timer.scheduled.replace(time)
        }

        fn cancel_timer(&mut self, timer: &mut Self::Timer) -> Option<Self::Instant> {
            timer.scheduled.take()
        }

        fn scheduled_instant(&self, timer: &mut Self::Timer) -> Option<Self::Instant> {
            timer.scheduled.clone()
        }

        fn unique_timer_id(&self, _: &Self::Timer) -> Self::UniqueTimerId {
            unimplemented!()
        }
    }

    #[derive(Default, Debug)]
    struct FakeTimer {
        scheduled: Option<FakeInstant>,
    }

    #[derive(Eq, PartialEq, Debug, Ord, PartialOrd, Copy, Clone, Hash)]
    struct TimerId(usize);

    type LocalTimerHeap = super::LocalTimerHeap<TimerId, (), FakeTimerCtx>;

    impl LocalTimerHeap {
        #[track_caller]
        fn assert_heap_entries<I: IntoIterator<Item = (FakeInstant, TimerId)>>(&self, i: I) {
            let mut want = i.into_iter().collect::<Vec<_>>();
            want.sort();
            let mut got = self
                .heap
                .heap
                .iter()
                .map(|HeapEntry { time, key }| (*time, *key))
                .collect::<Vec<_>>();
            got.sort();
            assert_eq!(got, want);
        }

        #[track_caller]
        fn assert_map_entries<I: IntoIterator<Item = (FakeInstant, TimerId)>>(&self, i: I) {
            let want = i.into_iter().map(|(t, k)| (k, t)).collect::<HashMap<_, _>>();
            let got = self
                .heap
                .map
                .iter()
                .map(|(k, MapEntry { time, value: () })| (*k, *time))
                .collect::<HashMap<_, _>>();
            assert_eq!(got, want);
        }
    }

    const TIMER1: TimerId = TimerId(1);
    const TIMER2: TimerId = TimerId(2);
    const TIMER3: TimerId = TimerId(3);

    const T1: FakeInstant = FakeInstant { offset: Duration::from_secs(1) };
    const T2: FakeInstant = FakeInstant { offset: Duration::from_secs(2) };
    const T3: FakeInstant = FakeInstant { offset: Duration::from_secs(3) };
    const T4: FakeInstant = FakeInstant { offset: Duration::from_secs(4) };

    #[test]
    fn schedule_instant() {
        let mut ctx = FakeTimerCtx::default();
        let mut heap = LocalTimerHeap::new(&mut ctx, ());
        assert_eq!(heap.next_wakeup.scheduled, None);
        heap.assert_heap_entries([]);

        assert_eq!(heap.schedule_instant(&mut ctx, TIMER2, (), T2), None);
        heap.assert_heap_entries([(T2, TIMER2)]);
        assert_eq!(heap.next_wakeup.scheduled, Some(T2));

        assert_eq!(heap.schedule_instant(&mut ctx, TIMER1, (), T1), None);
        heap.assert_heap_entries([(T1, TIMER1), (T2, TIMER2)]);
        assert_eq!(heap.next_wakeup.scheduled, Some(T1));

        assert_eq!(heap.schedule_instant(&mut ctx, TIMER3, (), T3), None);
        heap.assert_heap_entries([(T1, TIMER1), (T2, TIMER2), (T3, TIMER3)]);
        assert_eq!(heap.next_wakeup.scheduled, Some(T1));
    }

    #[test]
    fn schedule_after() {
        let mut ctx = FakeTimerCtx::default();
        let mut heap = LocalTimerHeap::new(&mut ctx, ());
        assert_eq!(heap.next_wakeup.scheduled, None);
        let long_duration = Duration::from_secs(5);
        let short_duration = Duration::from_secs(1);

        let long_instant = ctx.now().checked_add(long_duration).unwrap();
        let short_instant = ctx.now().checked_add(short_duration).unwrap();

        assert_eq!(heap.schedule_after(&mut ctx, TIMER1, (), long_duration), None);
        assert_eq!(heap.next_wakeup.scheduled, Some(long_instant));
        heap.assert_heap_entries([(long_instant, TIMER1)]);
        heap.assert_map_entries([(long_instant, TIMER1)]);

        assert_eq!(
            heap.schedule_after(&mut ctx, TIMER1, (), short_duration),
            Some((long_instant, ()))
        );
        assert_eq!(heap.next_wakeup.scheduled, Some(short_instant));
        heap.assert_heap_entries([(short_instant, TIMER1), (long_instant, TIMER1)]);
        heap.assert_map_entries([(short_instant, TIMER1)]);
    }

    #[test]
    fn cancel() {
        let mut ctx = FakeTimerCtx::default();
        let mut heap = LocalTimerHeap::new(&mut ctx, ());
        assert_eq!(heap.schedule_instant(&mut ctx, TIMER1, (), T1), None);
        assert_eq!(heap.schedule_instant(&mut ctx, TIMER2, (), T2), None);
        assert_eq!(heap.schedule_instant(&mut ctx, TIMER3, (), T3), None);
        heap.assert_heap_entries([(T1, TIMER1), (T2, TIMER2), (T3, TIMER3)]);
        assert_eq!(heap.next_wakeup.scheduled, Some(T1));

        assert_eq!(heap.cancel(&mut ctx, &TIMER1), Some((T1, ())));
        heap.assert_heap_entries([(T2, TIMER2), (T3, TIMER3)]);
        heap.assert_map_entries([(T2, TIMER2), (T3, TIMER3)]);
        assert_eq!(heap.next_wakeup.scheduled, Some(T2));

        assert_eq!(heap.cancel(&mut ctx, &TIMER1), None);

        assert_eq!(heap.cancel(&mut ctx, &TIMER3), Some((T3, ())));
        // Timer3 is still in the heap, hasn't had a chance to cleanup.
        heap.assert_heap_entries([(T2, TIMER2), (T3, TIMER3)]);
        heap.assert_map_entries([(T2, TIMER2)]);
        assert_eq!(heap.next_wakeup.scheduled, Some(T2));

        assert_eq!(heap.cancel(&mut ctx, &TIMER2), Some((T2, ())));
        heap.assert_heap_entries([]);
        heap.assert_map_entries([]);
        assert_eq!(heap.next_wakeup.scheduled, None);
    }

    #[test]
    fn pop() {
        let mut ctx = FakeTimerCtx::default();
        let mut heap = LocalTimerHeap::new(&mut ctx, ());
        assert_eq!(heap.schedule_instant(&mut ctx, TIMER1, (), T1), None);
        assert_eq!(heap.schedule_instant(&mut ctx, TIMER2, (), T2), None);
        assert_eq!(heap.schedule_instant(&mut ctx, TIMER3, (), T3), None);
        heap.assert_heap_entries([(T1, TIMER1), (T2, TIMER2), (T3, TIMER3)]);
        heap.assert_map_entries([(T1, TIMER1), (T2, TIMER2), (T3, TIMER3)]);
        assert_eq!(heap.next_wakeup.scheduled, Some(T1));

        assert_eq!(heap.pop(&mut ctx), None);
        heap.assert_heap_entries([(T1, TIMER1), (T2, TIMER2), (T3, TIMER3)]);
        heap.assert_map_entries([(T1, TIMER1), (T2, TIMER2), (T3, TIMER3)]);
        assert_eq!(heap.next_wakeup.scheduled, Some(T1));

        ctx.instant.time = T1;
        assert_eq!(heap.pop(&mut ctx), Some((TIMER1, ())));
        heap.assert_heap_entries([(T2, TIMER2), (T3, TIMER3)]);
        heap.assert_map_entries([(T2, TIMER2), (T3, TIMER3)]);
        assert_eq!(heap.next_wakeup.scheduled, Some(T2));
        assert_eq!(heap.pop(&mut ctx), None);
        assert_eq!(heap.next_wakeup.scheduled, Some(T2));

        ctx.instant.time = T3;
        assert_eq!(heap.pop(&mut ctx), Some((TIMER2, ())));
        heap.assert_heap_entries([(T3, TIMER3)]);
        heap.assert_map_entries([(T3, TIMER3)]);

        assert_eq!(heap.next_wakeup.scheduled, Some(T3));
        assert_eq!(heap.pop(&mut ctx), Some((TIMER3, ())));
        heap.assert_heap_entries([]);
        heap.assert_map_entries([]);
        assert_eq!(heap.next_wakeup.scheduled, None);

        assert_eq!(heap.pop(&mut ctx), None);
    }

    #[test]
    fn reschedule() {
        let mut ctx = FakeTimerCtx::default();
        let mut heap = LocalTimerHeap::new(&mut ctx, ());
        assert_eq!(heap.schedule_instant(&mut ctx, TIMER1, (), T1), None);
        assert_eq!(heap.schedule_instant(&mut ctx, TIMER2, (), T2), None);
        assert_eq!(heap.schedule_instant(&mut ctx, TIMER3, (), T3), None);
        heap.assert_heap_entries([(T1, TIMER1), (T2, TIMER2), (T3, TIMER3)]);
        heap.assert_map_entries([(T1, TIMER1), (T2, TIMER2), (T3, TIMER3)]);
        assert_eq!(heap.next_wakeup.scheduled, Some(T1));

        assert_eq!(heap.schedule_instant(&mut ctx, TIMER2, (), T4), Some((T2, ())));
        heap.assert_heap_entries([(T1, TIMER1), (T2, TIMER2), (T3, TIMER3)]);
        heap.assert_map_entries([(T1, TIMER1), (T4, TIMER2), (T3, TIMER3)]);

        ctx.instant.time = T4;
        // Popping TIMER1 makes the heap entry update.
        assert_eq!(heap.pop(&mut ctx), Some((TIMER1, ())));
        heap.assert_heap_entries([(T4, TIMER2), (T3, TIMER3)]);
        heap.assert_map_entries([(T4, TIMER2), (T3, TIMER3)]);
        assert_eq!(heap.next_wakeup.scheduled, Some(T3));

        assert_eq!(heap.schedule_instant(&mut ctx, TIMER2, (), T2), Some((T4, ())));
        heap.assert_heap_entries([(T2, TIMER2), (T4, TIMER2), (T3, TIMER3)]);
        heap.assert_map_entries([(T2, TIMER2), (T3, TIMER3)]);
        assert_eq!(heap.next_wakeup.scheduled, Some(T2));

        assert_eq!(heap.pop(&mut ctx), Some((TIMER2, ())));
        // Still has stale TIMER2 entry.
        heap.assert_heap_entries([(T4, TIMER2), (T3, TIMER3)]);
        heap.assert_map_entries([(T3, TIMER3)]);
        assert_eq!(heap.next_wakeup.scheduled, Some(T3));

        assert_eq!(heap.pop(&mut ctx), Some((TIMER3, ())));
        heap.assert_heap_entries([]);
        heap.assert_map_entries([]);
        assert_eq!(heap.next_wakeup.scheduled, None);
        assert_eq!(heap.pop(&mut ctx), None);
    }

    // Regression test for a bug where the timer heap would not reschedule the
    // next wakeup when it has two timers for the exact same instant at the top.
    #[test]
    fn multiple_timers_same_instant() {
        let mut ctx = FakeTimerCtx::default();
        let mut heap = LocalTimerHeap::new(&mut ctx, ());
        assert_eq!(heap.schedule_instant(&mut ctx, TIMER1, (), T1), None);
        assert_eq!(heap.schedule_instant(&mut ctx, TIMER2, (), T1), None);
        assert_eq!(heap.next_wakeup.scheduled.take(), Some(T1));

        ctx.instant.time = T1;

        // Ordering is not guaranteed, just assert that we're getting timers.
        assert!(heap.pop(&mut ctx).is_some());
        assert_eq!(heap.next_wakeup.scheduled, Some(T1));
        assert!(heap.pop(&mut ctx).is_some());
        assert_eq!(heap.next_wakeup.scheduled, None);
        assert_eq!(heap.pop(&mut ctx), None);
    }

    #[test]
    fn clear() {
        let mut ctx = FakeTimerCtx::default();
        let mut heap = LocalTimerHeap::new(&mut ctx, ());
        assert_eq!(heap.schedule_instant(&mut ctx, TIMER1, (), T1), None);
        assert_eq!(heap.schedule_instant(&mut ctx, TIMER2, (), T1), None);
        heap.clear(&mut ctx);
        heap.assert_map_entries([]);
        assert_eq!(heap.next_wakeup.scheduled, None);
    }
}
