// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Support for creating futures that represent timers.
//!
//! This module contains the `Timer` type which is a future that will resolve
//! at a particular point in the future.

use super::executor::Executor;
use crate::runtime::{BootInstant, EHandle, MonotonicInstant, WakeupTime};
use crate::{PacketReceiver, ReceiverRegistration};
use fuchsia_sync::Mutex;

use futures::future::FusedFuture;
use futures::stream::FusedStream;
use futures::task::{AtomicWaker, Context};
use futures::{FutureExt, Stream};
use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::{Poll, Waker, ready};

pub trait TimeInterface:
    Clone + Copy + fmt::Debug + PartialEq + PartialOrd + Ord + Send + Sync + 'static
{
    type Timeline: zx::Timeline + Send + Sync + 'static;

    fn from_nanos(nanos: i64) -> Self;
    fn into_nanos(self) -> i64;
    fn zx_instant(nanos: i64) -> zx::Instant<Self::Timeline>;
    fn now() -> i64;
    fn create_timer() -> zx::Timer<Self::Timeline>;
}

impl TimeInterface for MonotonicInstant {
    type Timeline = zx::MonotonicTimeline;

    fn from_nanos(nanos: i64) -> Self {
        Self::from_nanos(nanos)
    }

    fn into_nanos(self) -> i64 {
        self.into_nanos()
    }

    fn zx_instant(nanos: i64) -> zx::MonotonicInstant {
        zx::MonotonicInstant::from_nanos(nanos)
    }

    fn now() -> i64 {
        EHandle::local().inner().now().into_nanos()
    }

    fn create_timer() -> zx::Timer<Self::Timeline> {
        zx::Timer::<Self::Timeline>::create()
    }
}

impl TimeInterface for BootInstant {
    type Timeline = zx::BootTimeline;

    fn from_nanos(nanos: i64) -> Self {
        Self::from_nanos(nanos)
    }

    fn into_nanos(self) -> i64 {
        self.into_nanos()
    }

    fn zx_instant(nanos: i64) -> zx::BootInstant {
        zx::BootInstant::from_nanos(nanos)
    }

    fn now() -> i64 {
        EHandle::local().inner().boot_now().into_nanos()
    }

    fn create_timer() -> zx::Timer<Self::Timeline> {
        zx::Timer::<Self::Timeline>::create()
    }
}

impl WakeupTime for std::time::Instant {
    fn into_timer(self) -> Timer {
        let now_as_instant = std::time::Instant::now();
        let now_as_time = MonotonicInstant::now();
        EHandle::local()
            .mono_timers()
            .new_timer(now_as_time + self.saturating_duration_since(now_as_instant).into())
    }
}

impl WakeupTime for MonotonicInstant {
    fn into_timer(self) -> Timer {
        EHandle::local().mono_timers().new_timer(self)
    }
}

impl WakeupTime for BootInstant {
    fn into_timer(self) -> Timer {
        EHandle::local().boot_timers().new_timer(self)
    }
}

impl WakeupTime for zx::MonotonicInstant {
    fn into_timer(self) -> Timer {
        EHandle::local().mono_timers().new_timer(self.into())
    }
}

impl WakeupTime for zx::BootInstant {
    fn into_timer(self) -> Timer {
        EHandle::local().boot_timers().new_timer(self.into())
    }
}

/// An asynchronous timer.
#[must_use = "futures do nothing unless polled"]
pub struct Timer(TimerState);

impl Timer {
    /// Create a new timer scheduled to fire at `time`.
    pub fn new(time: impl WakeupTime) -> Self {
        time.into_timer()
    }

    /// Reset the `Timer` to a fire at a new time.
    pub fn reset(self: Pin<&mut Self>, time: MonotonicInstant) {
        let nanos = time.into_nanos();
        // If in the UNREGISTERED state, we can skip the call to `try_reset_timer` because the timer
        // has *never* been registered and so there's no danger of another thread having access to
        // this timer. In all other states, including the FIRED and TERMINATED states, we must use
        // `try_reset_timer` as that will take a lock and guarantee that other threads are not
        // concurrently accessing the timer.
        //
        // This can be Relaxed because because there are no loads or stores that follow that could
        // possibly be reordered before here that matter: the first thing `try_reset_timer` does is
        // take a lock which will have its own memory barriers, and the store to the time is next
        // going to be read by this same task prior to taking the lock in `Timers::inner`.
        if self.0.state.load(Ordering::Relaxed) == UNREGISTERED
            || !self.0.timers.try_reset_timer(&self.0, nanos)
        {
            // SAFETY: This is safe because we know the timer isn't registered which means we truly
            // have exclusive access to TimerState.
            unsafe { *self.0.nanos.get() = nanos };
            self.0.state.store(UNREGISTERED, Ordering::Relaxed);
        }
    }
}

impl fmt::Debug for Timer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        f.debug_struct("Timer").field("time", &self.0.nanos).finish()
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.0.timers.unregister(&self.0);
    }
}

impl Future for Timer {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: We call `unregister` when `Timer` is dropped.
        unsafe { self.0.timers.poll(self.as_ref(), cx) }
    }
}

struct TimerState {
    timers: Arc<dyn TimersInterface>,

    // This is safe to access/mutate if the lock on `Timers::inner` is held.
    nanos: UnsafeCell<i64>,

    waker: AtomicWaker,
    state: AtomicU8,

    // Holds the index in the heap.  This is safe to access/mutate if the lock on `Timers::inner` is
    // held.
    index: UnsafeCell<HeapIndex>,

    // StateRef stores a pointer to `TimerState`, so this must be pinned.
    _pinned: PhantomPinned,
}

// SAFETY: TimerState is thread-safe.  See the safety comments elsewhere.
unsafe impl Send for TimerState {}
unsafe impl Sync for TimerState {}

// Set when the timer is not registered in the heap.
const UNREGISTERED: u8 = 0;

// Set when the timer is in the heap.
const REGISTERED: u8 = 1;

// Set when the timer has fired.
const FIRED: u8 = 2;

// Set when the timer is terminated.
const TERMINATED: u8 = 3;

/// An index in the heap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeapIndex(usize);

impl HeapIndex {
    const NULL: HeapIndex = HeapIndex(usize::MAX);

    fn get(&self) -> Option<usize> {
        if *self == HeapIndex::NULL { None } else { Some(self.0) }
    }
}

impl From<usize> for HeapIndex {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl FusedFuture for Timer {
    fn is_terminated(&self) -> bool {
        self.0.state.load(Ordering::Relaxed) == TERMINATED
    }
}

// A note on safety:
//
//  1. We remove the timer from the heap before we drop TimerState, and TimerState is pinned, so
//     it's safe to store pointers in the heap i.e. the pointers are live since we make sure we
//     remove them before dropping `TimerState`.
//
//  2. Provided we do #1, it is always safe to access the atomic fields of TimerState.
//
//  3. Once the timer has been registered, it is safe to access the non-atomic fields of TimerState
//     whilst holding the lock on `Timers::inner`.
#[derive(Copy, Clone, Debug)]
struct StateRef(*const TimerState);

// SAFETY: See the notes above regarding safety.
unsafe impl Send for StateRef {}
unsafe impl Sync for StateRef {}

impl StateRef {
    fn into_waker(self, _inner: &mut Inner) -> Option<Waker> {
        // SAFETY: `inner` is locked.
        unsafe {
            // As soon as we set the state to FIRED, the heap no longer owns the timer and it might
            // be re-registered.  This store is safe to be Relaxed because `AtomicWaker::take` has a
            // Release barrier, so the store can't be reordered after it, and therefore we can be
            // certain that another thread which re-registers the waker will see the state is FIRED
            // (and will interpret that as meaning that the task should not block and instead
            // immediately complete; see `Timers::poll`).
            (*self.0).state.store(FIRED, Ordering::Relaxed);
            (*self.0).waker.take()
        }
    }

    // # Safety
    //
    // `Timers::inner` must be locked.
    unsafe fn nanos(&self) -> i64 {
        unsafe { *(*self.0).nanos.get() }
    }

    // # Safety
    //
    // `Timers::inner` must be locked.
    unsafe fn nanos_mut(&mut self) -> &mut i64 {
        unsafe { &mut *(*self.0).nanos.get() }
    }

    // # Safety
    //
    // `Timers::inner` must be locked.
    unsafe fn set_index(&mut self, index: HeapIndex) -> HeapIndex {
        std::mem::replace(unsafe { &mut *(*self.0).index.get() }, index)
    }
}

/// An asynchronous interval timer.
///
/// This is a stream of events resolving at a rate of once-per interval.  This generates an event
/// for *every* elapsed duration, even if multiple have elapsed since last polled.
///
/// TODO(https://fxbug.dev/375632319): This is lack of BootInstant support.
#[derive(Debug)]
#[must_use = "streams do nothing unless polled"]
pub struct Interval {
    timer: Pin<Box<Timer>>,
    next: MonotonicInstant,
    duration: zx::MonotonicDuration,
}

impl Interval {
    /// Create a new `Interval` which yields every `duration`.
    pub fn new(duration: zx::MonotonicDuration) -> Self {
        let next = MonotonicInstant::after(duration);
        Interval { timer: Box::pin(Timer::new(next)), next, duration }
    }
}

impl FusedStream for Interval {
    fn is_terminated(&self) -> bool {
        // `Interval` never yields `None`
        false
    }
}

impl Stream for Interval {
    type Item = ();
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        ready!(self.timer.poll_unpin(cx));
        let next = self.next + self.duration;
        self.timer.as_mut().reset(next);
        self.next = next;
        Poll::Ready(Some(()))
    }
}

pub(crate) struct Timers<T: TimeInterface> {
    inner: Mutex<Inner>,

    fake: bool,

    timer: zx::Timer<T::Timeline>,

    // This will form a reference cycle which the caller *must* break by calling `deregister`.
    receiver_registration: Mutex<Option<ReceiverRegistration<Arc<Self>>>>,
}

struct Inner {
    // The queue of active timer objects.
    timers: Heap,

    // The last deadline we set on the zircon timer, or None if the queue was empty and the timer
    // was canceled most recently.
    last_deadline: Option<i64>,

    // True if there's a pending async_wait.
    async_wait: bool,

    // The port key.
    port_key: u64,
}

impl<T: TimeInterface> Timers<T> {
    pub fn new(fake: bool) -> Self {
        Self {
            inner: Mutex::new(Inner {
                timers: Heap::default(),
                last_deadline: None,
                async_wait: false,
                port_key: 0,
            }),
            fake,
            timer: T::create_timer(),
            receiver_registration: Mutex::default(),
        }
    }

    pub fn new_timer(self: &Arc<Self>, time: T) -> Timer {
        let nanos = time.into_nanos();
        Timer(TimerState {
            timers: self.clone(),
            nanos: UnsafeCell::new(nanos),
            waker: AtomicWaker::new(),
            state: AtomicU8::new(UNREGISTERED),
            index: UnsafeCell::new(HeapIndex::NULL),
            _pinned: PhantomPinned,
        })
    }

    /// Registers the timers to receive packets.  This will establish a reference cycle that
    /// the caller must break by calling `deregister`.
    pub fn register(self: &Arc<Self>, executor: &Arc<Executor>) {
        let key = self
            .receiver_registration
            .lock()
            .get_or_insert_with(|| executor.receivers.register(executor.clone(), self.clone()))
            .key();
        self.inner.lock().port_key = key;
    }

    /// Deregisters the timers and breaks the reference cycle.
    pub fn deregister(&self) {
        *self.receiver_registration.lock() = None;
    }

    /// Ensures the underlying Zircon timer has been correctly set or canceled after
    /// the queue has been updated.
    ///
    /// # Safety
    ///
    /// Callers must ensure that `self.inner` is locked before calling this method.
    fn setup_zircon_timer(&self, inner: &mut Inner, from_receive_packet: bool) {
        // Our new deadline is the deadline for the front of the queue, or no deadline (infinite) if
        // there is no front of the queue.
        let new_deadline = inner.timers.peek().map(|timer| unsafe { timer.nanos() });

        // If the effective deadline of the queue has changed, reprogram the zircon timer's
        // deadline.
        if new_deadline != inner.last_deadline {
            inner.last_deadline = new_deadline;
            match inner.last_deadline {
                Some(deadline) => {
                    self.timer.set(T::zx_instant(deadline), zx::Duration::ZERO).unwrap()
                }
                None => self.timer.cancel().unwrap(),
            }
        }

        // If this is being called while processing the timer packet from a previous async wait,
        // then clear the async wait flag.  This is the very last thing we need to do, so this async
        // wait operation is effectively over.
        if from_receive_packet {
            inner.async_wait = false;
        }

        // If we have a valid timeout, but we have no in-flight async wait operation, post a new
        // one.
        if inner.last_deadline.is_some() && !inner.async_wait {
            if self.fake {
                // Clear the signal used for fake timers so that we can use it to trigger
                // next time.
                self.timer.signal(zx::Signals::USER_0, zx::Signals::empty()).unwrap();
            }

            self.timer
                .wait_async(
                    EHandle::local().port(),
                    inner.port_key,
                    if self.fake { zx::Signals::USER_0 } else { zx::Signals::TIMER_SIGNALED },
                    zx::WaitAsyncOpts::empty(),
                )
                .unwrap();

            inner.async_wait = true;
        }
    }

    /// Wakes timers that should be firing now.  Returns true if any timers were woken.
    pub fn wake_timers(&self) -> bool {
        self.wake_timers_impl(false)
    }

    fn wake_timers_impl(&self, from_receive_packet: bool) -> bool {
        let now = T::now();
        let mut timers_woken = false;

        loop {
            let waker = {
                let mut inner = self.inner.lock();

                // SAFETY: `inner` is locked.
                let deadline = inner.timers.peek().map(|timer| unsafe { timer.nanos() });
                if deadline.is_some_and(|d| d <= now) {
                    let timer = inner.timers.pop().unwrap();
                    timer.into_waker(&mut inner)
                } else {
                    // We are now finished (one way or the other). Setup the underlying Zircon timer
                    // to reflect the new state of the queue, and break out of the loop.
                    //
                    // When processing a timer packet (from_receive_packet is true), it is very
                    // important that this always be the last thing we do before breaking out of the
                    // loop (dropping the lock in the process) for good.
                    //
                    // Failing to do this at the end can lead to the timer queue stalling.  Doing it
                    // early (when we are dispatching expired timers) can lead to an ever
                    // multiplying army of posted async waits.
                    //
                    // See https://g-issues.fuchsia.dev/issues/396173066 for details.
                    self.setup_zircon_timer(&mut inner, from_receive_packet);
                    break;
                }
            };
            if let Some(waker) = waker {
                waker.wake()
            }
            timers_woken = true;
        }
        timers_woken
    }

    /// Wakes the next timer and returns its time.
    pub fn wake_next_timer(&self) -> Option<T> {
        let (nanos, waker) = {
            let mut inner = self.inner.lock();
            let timer = inner.timers.pop()?;
            // SAFETY: `inner` is locked.
            let nanos = unsafe { timer.nanos() };
            (nanos, timer.into_waker(&mut inner))
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        Some(T::from_nanos(nanos))
    }

    /// Returns the next timer due to expire.
    pub fn next_timer(&self) -> Option<T> {
        // SAFETY: `inner` is locked.
        self.inner.lock().timers.peek().map(|state| T::from_nanos(unsafe { state.nanos() }))
    }

    /// If there's a timer ready, sends a notification to wake up the receiver.
    ///
    /// # Panics
    ///
    /// This will panic if we are not using fake time.
    pub fn maybe_notify(&self, now: T) {
        assert!(self.fake, "calling this function requires using fake time.");
        // SAFETY: `inner` is locked.
        if self
            .inner
            .lock()
            .timers
            .peek()
            .is_some_and(|state| unsafe { state.nanos() } <= now.into_nanos())
        {
            self.timer.signal(zx::Signals::empty(), zx::Signals::USER_0).unwrap();
        }
    }
}

impl<T: TimeInterface> PacketReceiver for Timers<T> {
    fn receive_packet(&self, _packet: zx::Packet) {
        self.wake_timers_impl(true);
    }
}

// See comments on the implementation below.
trait TimersInterface: Send + Sync + 'static {
    unsafe fn poll(&self, timer: Pin<&Timer>, cx: &mut Context<'_>) -> Poll<()>;
    fn unregister(&self, state: &TimerState);
    fn try_reset_timer(&self, timer: &TimerState, nanos: i64) -> bool;
}

impl<T: TimeInterface> TimersInterface for Timers<T> {
    // # Safety
    //
    // `unregister` must be called before `Timer` is dropped.
    unsafe fn poll(&self, timer: Pin<&Timer>, cx: &mut Context<'_>) -> Poll<()> {
        // See https://docs.rs/futures/0.3.5/futures/task/struct.AtomicWaker.html
        // for more information.
        // Quick check to avoid registration if already done.
        //
        // This is safe to be Relaxed because `AtomicWaker::register` has barriers which means that
        // the load further down can't be moved before registering the waker, which means we can't
        // miss the timer firing.  If the timer isn't registered, the time might have been reset but
        // that would have been by the same task, so there should be no ordering issue there.  If we
        // then try and register the timer, we take the lock on `inner` so there will be barriers
        // there.
        let state = timer.0.state.load(Ordering::Relaxed);

        if state == TERMINATED {
            return Poll::Ready(());
        }

        if state == FIRED {
            timer.0.state.store(TERMINATED, Ordering::Relaxed);
            return Poll::Ready(());
        }

        if state == UNREGISTERED {
            // SAFETY: The state is UNREGISTERED, so we have exclusive access.
            let nanos = unsafe { *timer.0.nanos.get() };
            if nanos <= T::now() {
                timer.0.state.store(FIRED, Ordering::Relaxed);
                return Poll::Ready(());
            }
            let mut inner = self.inner.lock();

            // We store a pointer to `timer` here. This is safe to do because `timer` is pinned, and
            // we always make sure we call `unregister` before `timer` is dropped.
            inner.timers.push(StateRef(&timer.0));

            // Now that we have added a new timer to the queue, setup the
            // underlying zircon timer to reflect the new state of the queue.
            self.setup_zircon_timer(&mut inner, false);

            timer.0.state.store(REGISTERED, Ordering::Relaxed);
        }

        timer.0.waker.register(cx.waker());

        // Now that we've registered a waker, we need to check to see if the timer has been marked
        // as FIRED by another thread in the meantime (e.g. in StateRef::into_waker).  In that case
        // the timer is never going to fire again as it is no longer managed by the timer heap, so
        // the timer's task would become Pending but nothing would wake it up later.
        // Loading the state *must* happen after the above `AtomicWaker::register` (which
        // establishes an Acquire barrier, preventing the below load from being reordered above it),
        // or else we could racily hit the above scenario.
        let state = timer.0.state.load(Ordering::Relaxed);
        match state {
            FIRED => {
                timer.0.state.store(TERMINATED, Ordering::Relaxed);
                Poll::Ready(())
            }
            REGISTERED => Poll::Pending,
            // TERMINATED is only set in `poll` which has exclusive access to the task (&mut
            // Context).
            // UNREGISTERED would indicate a logic bug somewhere.
            _ => {
                unreachable!();
            }
        }
    }

    fn unregister(&self, timer: &TimerState) {
        if timer.state.load(Ordering::Relaxed) == UNREGISTERED {
            // If the timer was never registered, then we have exclusive access and we can skip the
            // rest of this (avoiding the lock on `inner`).
            // We cannot early-exit if the timer is FIRED or TERMINATED because then we could race
            // with another thread that is actively using the timer object, and if this call
            // completes before it blocks on `inner`, then the timer's resources could be
            // deallocated, which would result in a use-after-free on the other thread.
            return;
        }
        let mut inner = self.inner.lock();
        // SAFETY: `inner` is locked.
        let index = unsafe { *timer.index.get() };
        if let Some(index) = index.get() {
            inner.timers.remove(index);
            if index == 0 {
                // The front of the queue just changed.  Make sure to update the underlying zircon
                // timer state to match the new queue state.
                self.setup_zircon_timer(&mut inner, false);
            }
            timer.state.store(UNREGISTERED, Ordering::Relaxed);
        }
    }

    /// Returns true if the timer was successfully reset.
    fn try_reset_timer(&self, timer: &TimerState, nanos: i64) -> bool {
        let mut inner = self.inner.lock();
        // SAFETY: `inner` is locked.
        let index = unsafe { *timer.index.get() };
        if let Some(old_index) = index.get() {
            if (inner.timers.reset(old_index, nanos) == 0) || (old_index == 0) {
                // If the timer has moved into or out-of the front queue position, update the
                // underlying zircon timer to reflect the new queue state.
                self.setup_zircon_timer(&mut inner, false);
            }
            timer.state.store(REGISTERED, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

#[derive(Default)]
struct Heap(Vec<StateRef>);

// BinaryHeap doesn't support removal, and BTreeSet ends up increasing binary size significantly,
// so we roll our own binary heap.
impl Heap {
    fn push(&mut self, mut timer: StateRef) {
        let index = self.0.len();
        self.0.push(timer);
        // SAFETY: `inner` is locked.
        unsafe {
            timer.set_index(index.into());
        }
        self.fix_up(index);
    }

    fn peek(&self) -> Option<&StateRef> {
        self.0.first()
    }

    fn pop(&mut self) -> Option<StateRef> {
        if let Some(&first) = self.0.first() {
            self.remove(0);
            Some(first)
        } else {
            None
        }
    }

    fn swap(&mut self, a: usize, b: usize) {
        self.0.swap(a, b);
        // SAFETY: `inner` is locked.
        unsafe {
            self.0[a].set_index(a.into());
            self.0[b].set_index(b.into());
        }
    }

    /// Resets the timer at the given index to the new time and returns the new index.
    fn reset(&mut self, index: usize, nanos: i64) -> usize {
        // SAFETY: `inner` is locked.
        if nanos < std::mem::replace(unsafe { self.0[index].nanos_mut() }, nanos) {
            self.fix_up(index)
        } else {
            self.fix_down(index)
        }
    }

    fn remove(&mut self, index: usize) {
        // SAFETY: `inner` is locked.
        unsafe {
            let old_index = self.0[index].set_index(HeapIndex::NULL);
            debug_assert_eq!(old_index, index.into());
        }

        // Swap the item at slot `index` to the end of the vector so we can truncate it away, and
        // then swap the previously last item into the correct spot.
        let last = self.0.len() - 1;
        if index < last {
            let fix_up;
            unsafe {
                // SAFETY: `inner` is locked.
                fix_up = self.0[last].nanos() < self.0[index].nanos();
                self.0[index] = self.0[last];
                self.0[index].set_index(index.into());
            };
            self.0.truncate(last);
            if fix_up {
                self.fix_up(index);
            } else {
                self.fix_down(index);
            }
        } else {
            self.0.truncate(last);
        };
    }

    /// Returns the new index
    fn fix_up(&mut self, mut index: usize) -> usize {
        while index > 0 {
            let parent = (index - 1) / 2;
            // SAFETY: `inner` is locked.
            if unsafe { self.0[parent].nanos() <= self.0[index].nanos() } {
                return index;
            }
            self.swap(parent, index);
            index = parent;
        }
        index
    }

    /// Returns the new index
    fn fix_down(&mut self, mut index: usize) -> usize {
        let len = self.0.len();
        loop {
            let left = index * 2 + 1;
            if left >= len {
                return index;
            }

            let mut swap_with = None;

            // SAFETY: `inner` is locked.
            unsafe {
                let mut nanos = self.0[index].nanos();
                let left_nanos = self.0[left].nanos();
                if left_nanos < nanos {
                    swap_with = Some(left);
                    nanos = left_nanos;
                }
                let right = left + 1;
                if right < len && self.0[right].nanos() < nanos {
                    swap_with = Some(right);
                }
            }

            let Some(swap_with) = swap_with else { return index };
            self.swap(index, swap_with);
            index = swap_with;
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{LocalExecutor, SendExecutorBuilder, Task, TestExecutor};
    use assert_matches::assert_matches;
    use futures::channel::oneshot::channel;
    use futures::future::Either;
    use futures::prelude::*;
    use rand::seq::SliceRandom as _;
    use rand::{RngExt as _, rng};
    use std::future::poll_fn;
    use std::pin::pin;
    use zx::MonotonicDuration;

    trait TestTimeInterface:
        TimeInterface
        + WakeupTime
        + std::ops::Sub<zx::Duration<Self::Timeline>, Output = Self>
        + std::ops::Add<zx::Duration<Self::Timeline>, Output = Self>
    {
        fn after(duration: zx::Duration<Self::Timeline>) -> Self;
    }

    impl TestTimeInterface for MonotonicInstant {
        fn after(duration: zx::MonotonicDuration) -> Self {
            Self::after(duration)
        }
    }

    impl TestTimeInterface for BootInstant {
        fn after(duration: zx::BootDuration) -> Self {
            Self::after(duration)
        }
    }

    fn test_shorter_fires_first<T: TestTimeInterface>() {
        let mut exec = LocalExecutor::default();
        let shorter = pin!(Timer::new(T::after(zx::Duration::<T::Timeline>::from_millis(100))));
        let longer = pin!(Timer::new(T::after(zx::Duration::<T::Timeline>::from_seconds(1))));
        match exec.run_singlethreaded(future::select(shorter, longer)) {
            Either::Left(_) => {}
            Either::Right(_) => panic!("wrong timer fired"),
        }
    }

    #[test]
    fn shorter_fires_first() {
        test_shorter_fires_first::<MonotonicInstant>();
        test_shorter_fires_first::<BootInstant>();
    }

    fn test_shorter_fires_first_multithreaded<T: TestTimeInterface>() {
        SendExecutorBuilder::new().num_threads(4).build().run(async {
            let shorter = pin!(Timer::new(T::after(zx::Duration::<T::Timeline>::from_millis(100))));
            let longer = pin!(Timer::new(T::after(zx::Duration::<T::Timeline>::from_seconds(1))));
            match future::select(shorter, longer).await {
                Either::Left(_) => {}
                Either::Right(_) => panic!("wrong timer fired"),
            }
        });
    }

    #[test]
    fn shorter_fires_first_multithreaded() {
        test_shorter_fires_first_multithreaded::<MonotonicInstant>();
        test_shorter_fires_first_multithreaded::<BootInstant>();
    }

    fn test_timer_before_now_fires_immediately<T: TestTimeInterface>() {
        let mut exec = TestExecutor::new();
        let now = T::now();
        let before = pin!(Timer::new(T::from_nanos(now - 1)));
        let after = pin!(Timer::new(T::from_nanos(now + 1)));
        assert_matches!(
            exec.run_singlethreaded(futures::future::select(before, after)),
            Either::Left(_),
            "Timer in the past should fire first"
        );
    }

    #[test]
    fn timer_before_now_fires_immediately() {
        test_timer_before_now_fires_immediately::<MonotonicInstant>();
        test_timer_before_now_fires_immediately::<BootInstant>();
    }

    #[test]
    fn fires_after_timeout() {
        let mut exec = TestExecutor::new_with_fake_time();
        exec.set_fake_time(MonotonicInstant::from_nanos(0));
        let deadline = MonotonicInstant::after(MonotonicDuration::from_seconds(5));
        let mut future = pin!(Timer::new(deadline));
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut future));
        exec.set_fake_time(deadline);
        assert_eq!(Poll::Ready(()), exec.run_until_stalled(&mut future));
    }

    #[test]
    fn interval() {
        let mut exec = TestExecutor::new_with_fake_time();
        let start = MonotonicInstant::from_nanos(0);
        exec.set_fake_time(start);

        let counter = Arc::new(::std::sync::atomic::AtomicUsize::new(0));
        let mut future = pin!({
            let counter = counter.clone();
            Interval::new(MonotonicDuration::from_seconds(5))
                .map(move |()| {
                    counter.fetch_add(1, Ordering::SeqCst);
                })
                .collect::<()>()
        });

        // PollResult for the first time before the timer runs
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut future));
        assert_eq!(0, counter.load(Ordering::SeqCst));

        // Pretend to wait until the next timer
        let first_deadline = TestExecutor::next_timer().expect("Expected a pending timeout (1)");
        assert!(first_deadline >= MonotonicDuration::from_seconds(5) + start);
        exec.set_fake_time(first_deadline);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut future));
        assert_eq!(1, counter.load(Ordering::SeqCst));

        // PollResulting again before the timer runs shouldn't produce another item from the stream
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut future));
        assert_eq!(1, counter.load(Ordering::SeqCst));

        // "Wait" until the next timeout and poll again: expect another item from the stream
        let second_deadline = TestExecutor::next_timer().expect("Expected a pending timeout (2)");
        exec.set_fake_time(second_deadline);
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut future));
        assert_eq!(2, counter.load(Ordering::SeqCst));

        assert_eq!(second_deadline, first_deadline + MonotonicDuration::from_seconds(5));
    }

    #[test]
    fn timer_fake_time() {
        let mut exec = TestExecutor::new_with_fake_time();
        exec.set_fake_time(MonotonicInstant::from_nanos(0));

        let mut timer =
            pin!(Timer::new(MonotonicInstant::after(MonotonicDuration::from_seconds(1))));
        assert_eq!(Poll::Pending, exec.run_until_stalled(&mut timer));

        exec.set_fake_time(MonotonicInstant::after(MonotonicDuration::from_seconds(1)));
        assert_eq!(Poll::Ready(()), exec.run_until_stalled(&mut timer));
    }

    fn create_timers(
        timers: &Arc<Timers<MonotonicInstant>>,
        nanos: &[i64],
        timer_futures: &mut Vec<Pin<Box<Timer>>>,
    ) {
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        for &n in nanos {
            let mut timer = Box::pin(timers.new_timer(MonotonicInstant::from_nanos(n)));
            let _ = timer.poll_unpin(&mut cx);
            timer_futures.push(timer);
        }
    }

    #[test]
    fn timer_heap() {
        let _exec = TestExecutor::new_with_fake_time();
        let timers = Arc::new(Timers::<MonotonicInstant>::new(true));
        timers.register(EHandle::local().inner());

        let mut timer_futures = Vec::new();
        let mut nanos: Vec<_> = (0..1000).collect();
        let mut rng = rng();
        nanos.shuffle(&mut rng);

        create_timers(&timers, &nanos, &mut timer_futures);

        // Make sure the timers fire in the correct order.
        for i in 0..1000 {
            assert_eq!(timers.wake_next_timer(), Some(MonotonicInstant::from_nanos(i)));
        }

        timer_futures.clear();
        create_timers(&timers, &nanos, &mut timer_futures);

        // Remove half of them in random order, and ensure the remaining timers are correctly
        // ordered.
        timer_futures.shuffle(&mut rng);
        timer_futures.truncate(500);
        let mut last_time = None;
        for _ in 0..500 {
            let time = timers.wake_next_timer().unwrap();
            if let Some(last_time) = last_time {
                assert!(last_time <= time);
            }
            last_time = Some(time);
        }
        assert_eq!(timers.wake_next_timer(), None);

        timer_futures = vec![];
        create_timers(&timers, &nanos, &mut timer_futures);

        // Replace them all in random order.
        timer_futures.shuffle(&mut rng);
        let mut nanos: Vec<_> = (1000..2000).collect();
        nanos.shuffle(&mut rng);

        for (fut, n) in timer_futures.iter_mut().zip(nanos) {
            fut.as_mut().reset(MonotonicInstant::from_nanos(n));
        }

        // Check they all get changed and now fire in the correct order.
        for i in 1000..2000 {
            assert_eq!(timers.wake_next_timer(), Some(MonotonicInstant::from_nanos(i)));
        }

        timers.deregister();
    }

    #[test]
    fn timer_heap_with_same_time() {
        let _exec = TestExecutor::new_with_fake_time();
        let timers = Arc::new(Timers::<MonotonicInstant>::new(true));
        timers.register(EHandle::local().inner());

        let mut timer_futures = Vec::new();
        let mut nanos: Vec<_> = (1..100).collect();
        let mut rng = rng();
        nanos.shuffle(&mut rng);

        create_timers(&timers, &nanos, &mut timer_futures);

        // Create some timers with the same time.
        let time = rng.random_range(0..101);
        let same_time = [time; 100];
        create_timers(&timers, &same_time, &mut timer_futures);

        nanos.extend(&same_time);
        nanos.sort();

        for n in nanos {
            assert_eq!(timers.wake_next_timer(), Some(MonotonicInstant::from_nanos(n)));
        }

        timers.deregister();
    }

    #[test]
    fn timer_reset_to_earlier_time() {
        let mut exec = LocalExecutor::default();

        for _ in 0..100 {
            let instant = MonotonicInstant::after(MonotonicDuration::from_millis(100));
            let (sender, receiver) = channel();
            let task = Task::spawn(async move {
                let mut timer = pin!(Timer::new(instant));
                let mut receiver = pin!(receiver.fuse());
                poll_fn(|cx| {
                    loop {
                        if timer.as_mut().poll_unpin(cx).is_ready() {
                            return Poll::Ready(());
                        }
                        if !receiver.is_terminated() && receiver.poll_unpin(cx).is_ready() {
                            timer
                                .as_mut()
                                .reset(MonotonicInstant::after(MonotonicDuration::from_millis(1)));
                        } else {
                            return Poll::Pending;
                        }
                    }
                })
                .await;
            });
            sender.send(()).unwrap();

            exec.run_singlethreaded(task);

            if MonotonicInstant::after(MonotonicDuration::from_millis(1)) < instant {
                return;
            }
        }

        panic!("Timer fired late in all 100 attempts");
    }

    #[test]
    fn test_reset() {
        // This is a test for https://fxbug.dev/418235546.
        SendExecutorBuilder::new().num_threads(2).build().run(async {
            const TIMER_DELAY: zx::MonotonicDuration = zx::Duration::from_micros(100);
            let mut timer = pin!(Timer::new(MonotonicInstant::after(TIMER_DELAY)));
            for _ in 0..10000 {
                let _ = futures::poll!(timer.as_mut());
                std::thread::sleep(std::time::Duration::from_micros(rand::random_range(80..120)));
                timer.as_mut().reset(MonotonicInstant::after(TIMER_DELAY));
                timer.set(Timer::new(MonotonicInstant::after(TIMER_DELAY)));
            }
        });
    }
}
