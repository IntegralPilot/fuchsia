// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod hooks;
pub mod spawnable_future;

use crate::ScopeHandle;
use futures::ready;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

/// A lock-free thread-safe future.
//
// The debugger knows the layout so that async backtraces work, so if this changes the debugger
// might need to be changed too.
//
// This is `repr(C)` so that we can cast between `NonNull<Meta>` and `NonNull<AtomicFuture<F>>`.
//
// LINT.IfChange
#[repr(C)]
struct AtomicFuture<F: Future> {
    meta: Meta,

    // `future` is safe to access after successfully clearing the INACTIVE state bit and the `DONE`
    // state bit isn't set.
    future: FutureOrResult<F>,
}
// LINT.ThenChange(//src/developer/debug/zxdb/console/commands/verb_async_backtrace.cc)

/// A lock-free thread-safe future. The handles can be cloned.
#[derive(Debug)]
pub struct AtomicFutureHandle<'a>(NonNull<Meta>, PhantomData<&'a ()>);

/// `AtomicFutureHandle` is safe to access from multiple threads at once.
unsafe impl Sync for AtomicFutureHandle<'_> {}
unsafe impl Send for AtomicFutureHandle<'_> {}

impl Drop for AtomicFutureHandle<'_> {
    fn drop(&mut self) {
        self.meta().release();
    }
}

impl Clone for AtomicFutureHandle<'_> {
    fn clone(&self) -> Self {
        self.meta().retain();
        Self(self.0, PhantomData)
    }
}

impl PartialEq for AtomicFutureHandle<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for AtomicFutureHandle<'_> {}

impl Hash for AtomicFutureHandle<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

struct Meta {
    vtable: &'static VTable,

    // Holds the reference count and state bits (INACTIVE, READY, etc.).
    state: AtomicUsize,

    scope: Option<ScopeHandle>,
}

impl Meta {
    // # Safety
    //
    // This mints a handle with the 'static lifetime, so this should only be called from
    // `AtomicFutureHandle<'static>`.
    unsafe fn wake(&self) {
        if self.state.fetch_or(READY, Relaxed) & (INACTIVE | READY | DONE) == INACTIVE {
            self.retain();
            self.scope().executor().task_is_ready(AtomicFutureHandle(self.into(), PhantomData));
        }
    }

    // Returns true if a guard should be acquired.
    //
    // # Safety
    //
    // This mints a handle with the 'static lifetime, so this should only be called from
    // `AtomicFutureHandle<'static>`.
    unsafe fn wake_with_active_guard(&self) -> bool {
        let old = self.state.fetch_or(READY | WITH_ACTIVE_GUARD, Relaxed);
        if old & (INACTIVE | READY | DONE) == INACTIVE {
            self.retain();
            self.scope().executor().task_is_ready(AtomicFutureHandle(self.into(), PhantomData));
        }

        // If the task is DONE, the guard won't be released, so we must let the caller know.
        old & (DONE | WITH_ACTIVE_GUARD) == 0
    }

    fn scope(&self) -> &ScopeHandle {
        self.scope.as_ref().unwrap()
    }

    fn retain(&self) {
        let old = self.state.fetch_add(1, Relaxed) & REF_COUNT_MASK;
        assert!(old != REF_COUNT_MASK);
    }

    fn release(&self) {
        // This can be Relaxed because there is a barrier in the drop function.
        let old = self.state.fetch_sub(1, Relaxed) & REF_COUNT_MASK;
        if old == 1 {
            // SAFETY: This is safe because we just released the last reference.
            unsafe {
                (self.vtable.drop)(self.into());
            }
        } else {
            // Check for underflow.
            assert!(old > 0);
        }
    }

    // # Safety
    //
    // The caller must know that the future has completed.
    unsafe fn drop_result(&self, ordering: Ordering) {
        // It's possible for this to race with another thread so we only drop the result if we are
        // successful in setting the RESULT_TAKEN bit.
        if self.state.fetch_or(RESULT_TAKEN, ordering) & RESULT_TAKEN == 0 {
            unsafe { (self.vtable.drop_result)(self.into()) };
        }
    }
}

struct VTable {
    /// Drops the atomic future.
    ///
    /// # Safety
    ///
    /// The caller must ensure there are no other references i.e. the reference count should be
    /// zero.
    // zxdb uses this method to figure out the concrete type of the future.
    // LINT.IfChange
    drop: unsafe fn(NonNull<Meta>),
    // LINT.ThenChange(//src/developer/debug/zxdb/console/commands/verb_async_backtrace.cc)
    /// Drops the future.
    ///
    /// # Safety
    ///
    /// The caller must ensure the future hasn't been dropped.
    drop_future: unsafe fn(NonNull<Meta>),
    /// Polls the future.
    ///
    /// # Safety
    ///
    /// The caller must ensure the future hasn't been dropped and has exclusive access.
    poll: unsafe fn(NonNull<Meta>, cx: &mut Context<'_>) -> Poll<()>,

    /// Gets the result.
    ///
    /// # Safety
    ///
    /// The caller must ensure the future is finished and the result hasn't been taken or dropped.
    get_result: unsafe fn(NonNull<Meta>) -> *const (),

    /// Drops the result.
    ///
    /// # Safety
    ///
    /// The caller must ensure the future is finished and the result hasn't already been taken or
    /// dropped.
    drop_result: unsafe fn(NonNull<Meta>),
}

union FutureOrResult<F: Future> {
    future: ManuallyDrop<F>,
    result: ManuallyDrop<F::Output>,
}

impl<F: Future> AtomicFuture<F> {
    const VTABLE: VTable = VTable {
        drop: Self::drop,
        drop_future: Self::drop_future,
        poll: Self::poll,
        get_result: Self::get_result,
        drop_result: Self::drop_result,
    };

    unsafe fn drop(meta: NonNull<Meta>) {
        drop(unsafe { Box::from_raw(meta.cast::<Self>().as_mut()) });
    }

    unsafe fn poll(meta: NonNull<Meta>, cx: &mut Context<'_>) -> Poll<()> {
        let future = &mut unsafe { meta.cast::<Self>().as_mut() }.future;
        let result = ready!(unsafe { Pin::new_unchecked(&mut *future.future) }.poll(cx));
        // This might panic which will leave ourselves in a bad state. We deal with this by
        // aborting (see below).
        unsafe { ManuallyDrop::drop(&mut future.future) };
        future.result = ManuallyDrop::new(result);
        Poll::Ready(())
    }

    unsafe fn drop_future(meta: NonNull<Meta>) {
        unsafe { ManuallyDrop::drop(&mut meta.cast::<Self>().as_mut().future.future) };
    }

    unsafe fn get_result(meta: NonNull<Meta>) -> *const () {
        unsafe { &*meta.cast::<Self>().as_mut().future.result as *const F::Output as *const () }
    }

    unsafe fn drop_result(meta: NonNull<Meta>) {
        unsafe { ManuallyDrop::drop(&mut meta.cast::<Self>().as_mut().future.result) };
    }
}

/// State Bits
//
// Exclusive access is gained by clearing this bit.
const INACTIVE: usize = 1 << 63;

// Set to indicate the future needs to be polled again.
const READY: usize = 1 << 62;

// Terminal state: the future is dropped upon entry to this state. When in this state, other bits
// can be set, including READY (which has no meaning).
const DONE: usize = 1 << 61;

// The task has been detached.
const DETACHED: usize = 1 << 60;

// The task has been cancelled.
const ABORTED: usize = 1 << 59;

// The task has an active guard that should be dropped when the task is next polled.
const WITH_ACTIVE_GUARD: usize = 1 << 58;

// The result has been taken.
const RESULT_TAKEN: usize = 1 << 57;

// The task is low priority.
const LOW_PRIORITY: usize = 1 << 56;

// The mask for the ref count.
const REF_COUNT_MASK: usize = LOW_PRIORITY - 1;

/// The result of a call to `try_poll`.
/// This indicates the result of attempting to `poll` the future.
pub enum AttemptPollResult {
    /// The future was polled, but did not complete.
    Pending,
    /// The future was polled and finished by this thread.
    /// This result is normally used to trigger garbage-collection of the future.
    IFinished,
    /// The future was already completed by another thread.
    SomeoneElseFinished,
    /// The future was polled, did not complete, but it is woken whilst it is polled so it
    /// should be polled again.
    Yield,
    /// The future was aborted.
    Aborted,
}

/// The result of calling the `abort_and_detach` function.
#[must_use]
pub enum AbortAndDetachResult {
    /// The future has finished; it can be dropped.
    Done,

    /// The future needs to be added to a run queue to be aborted.
    AddToRunQueue,

    /// The future is soon to be aborted and nothing needs to be done.
    Pending,
}

impl<'a> AtomicFutureHandle<'a> {
    /// Create a new `AtomicFuture`.
    pub(crate) fn new<F: Future + Send + 'a>(scope: Option<ScopeHandle>, future: F) -> Self
    where
        F::Output: Send + 'a,
    {
        // SAFETY: This is safe because the future and output are both Send.
        unsafe { Self::new_local(scope, future) }
    }

    /// Create a new `AtomicFuture` from a !Send future.
    ///
    /// # Safety
    ///
    /// The caller must uphold the Send requirements.
    pub(crate) unsafe fn new_local<F: Future + 'a>(scope: Option<ScopeHandle>, future: F) -> Self
    where
        F::Output: 'a,
    {
        Self(
            NonNull::from_mut(Box::leak(Box::new(AtomicFuture {
                meta: Meta {
                    vtable: &AtomicFuture::<F>::VTABLE,
                    // The future is inactive and we start with a single reference.
                    state: AtomicUsize::new(1 | INACTIVE),
                    scope,
                },
                future: FutureOrResult { future: ManuallyDrop::new(future) },
            })))
            .cast::<Meta>(),
            PhantomData,
        )
    }

    fn meta(&self) -> &Meta {
        // SAFETY: This is safe because we hold a reference count.
        unsafe { self.0.as_ref() }
    }

    /// Returns the future's ID.
    ///
    /// The ID is only valid so long as there exists at least one live handle.
    pub fn id(&self) -> usize {
        // We use the address of the metadata as the ID since we know it's a stable heap address.
        // We can't use Pin to guarantee it never moves because the actual pointer to the
        // AtomicFuture is stored as a NonNull<Meta>.
        //
        // See https://github.com/rust-lang/rust/issues/54815 for an upstream feature request that
        // would let us encode this in the types.
        self.meta() as *const Meta as usize
    }

    /// Returns the associated scope.
    pub fn scope(&self) -> &ScopeHandle {
        self.meta().scope()
    }

    /// Attempt to poll the underlying future.
    ///
    /// `try_poll` ensures that the future is polled at least once more
    /// unless it has already finished.
    pub(crate) fn try_poll(&self, cx: &mut Context<'_>) -> AttemptPollResult {
        let meta = self.meta();
        let has_active_guard = loop {
            // Attempt to acquire sole responsibility for polling the future (by clearing the
            // INACTIVE bit) and also clear the READY and WITH_ACTIVE_GUARD bits at the same time.
            // We clear both so that we can track if they are set again whilst we are polling.
            let old = meta.state.fetch_and(!(INACTIVE | READY | WITH_ACTIVE_GUARD), Acquire);
            assert_ne!(old & REF_COUNT_MASK, 0);
            if old & DONE != 0 {
                // If the DONE bit is set, the WITH_ACTIVE_GUARD bit should be ignored; it may or
                // may not be set, but it doesn't reflect whether an active guard is held so even
                // though we just cleared it, we shouldn't release a guard here.
                return AttemptPollResult::SomeoneElseFinished;
            }
            let has_active_guard = old & WITH_ACTIVE_GUARD != 0;
            if old & INACTIVE != 0 {
                // We are now the (only) active worker, proceed to poll...
                if old & ABORTED != 0 {
                    if has_active_guard {
                        meta.scope().release_cancel_guard();
                    }
                    // The future was aborted.
                    // SAFETY: We have exclusive access.
                    unsafe {
                        self.drop_future_unchecked();
                    }
                    return AttemptPollResult::Aborted;
                }
                break has_active_guard;
            }
            // Future was already active; this shouldn't really happen because we shouldn't be
            // polling it from multiple threads at the same time. Still, we handle it by setting
            // the READY bit so that it gets polled again. We do this regardless of whether we
            // cleared the READY bit above.
            let old2 = meta.state.fetch_or(READY | (old & WITH_ACTIVE_GUARD), Relaxed);

            if old2 & DONE != 0 {
                // If `has_active_guard` is true, we are responsible for releasing a guard since it
                // means we cleared the `WITH_ACTIVE_GUARD` bit.
                if has_active_guard {
                    meta.scope().release_cancel_guard();
                }
                return AttemptPollResult::SomeoneElseFinished;
            }

            if has_active_guard && old2 & WITH_ACTIVE_GUARD != 0 {
                // Within the small window, something else gave this task an active guard, so we
                // must return one of them.
                meta.scope().release_cancel_guard();
            }

            // If the future is still active, or the future was already marked as ready, we can
            // just return and it will get polled again.
            if old2 & INACTIVE == 0 || old2 & READY != 0 {
                return AttemptPollResult::Pending;
            }
            // The worker finished, and we marked the future as ready, so we must try again because
            // the future won't be in a run queue.
        };

        // We cannot recover from panics.
        let bomb = Bomb;

        // SAFETY: We have exclusive access because we cleared the INACTIVE state bit.
        let result = unsafe { (meta.vtable.poll)(meta.into(), cx) };

        std::mem::forget(bomb);

        if has_active_guard {
            meta.scope().release_cancel_guard();
        }

        if let Poll::Ready(()) = result {
            // The future will have been dropped, so we just need to set the state.
            //
            // This needs to be Release ordering because we need to synchronize with another thread
            // that takes or drops the result.
            let old = meta.state.fetch_or(DONE, Release);

            if old & WITH_ACTIVE_GUARD != 0 {
                // Whilst we were polling the task, it was given an active guard. We must return it
                // now.
                meta.scope().release_cancel_guard();
            }

            if old & DETACHED != 0 {
                // If the future is detached, we should eagerly drop the result. This can be
                // Relaxed ordering because the result was written by this thread.

                // SAFETY: The future has completed.
                unsafe {
                    meta.drop_result(Relaxed);
                }
            }
            // No one else will read `future` unless they see `INACTIVE`, which will never
            // happen again.
            AttemptPollResult::IFinished
        } else if meta.state.fetch_or(INACTIVE, Release) & READY == 0 {
            AttemptPollResult::Pending
        } else {
            // The future was marked ready whilst we were polling, so yield.
            AttemptPollResult::Yield
        }
    }

    /// Drops the future without checking its current state.
    ///
    /// # Panics
    ///
    /// This will panic if the future is already marked with `DONE`.
    ///
    /// # Safety
    ///
    /// This doesn't check the current state, so this must only be called if it is known that there
    /// is no concurrent access. This also does *not* include any memory barriers before dropping
    /// the future.
    pub(crate) unsafe fn drop_future_unchecked(&self) {
        // Set the state first in case we panic when we drop.
        let meta = self.meta();
        let old = meta.state.fetch_or(DONE | RESULT_TAKEN, Relaxed);
        assert_eq!(old & DONE, 0);
        if old & WITH_ACTIVE_GUARD != 0 {
            meta.scope().release_cancel_guard();
        }
        unsafe { (meta.vtable.drop_future)(meta.into()) };
    }

    /// Drops the future if it is not currently being polled. Returns success if the future was
    /// dropped or was already dropped.
    pub(crate) fn try_drop(&self) -> Result<(), ()> {
        let old = self.meta().state.fetch_and(!INACTIVE, Acquire);
        if old & DONE != 0 {
            Ok(())
        } else if old & INACTIVE != 0 {
            // SAFETY: We have exclusive access.
            unsafe {
                self.drop_future_unchecked();
            }
            Ok(())
        } else {
            Err(())
        }
    }

    /// Aborts the task. Returns true if the task needs to be added to a run queue.
    #[must_use]
    pub(crate) fn abort(&self) -> bool {
        self.meta().state.fetch_or(ABORTED | READY, Relaxed) & (INACTIVE | READY | DONE) == INACTIVE
    }

    /// Marks the task as detached.
    pub(crate) fn detach(&self) {
        let meta = self.meta();
        let old = meta.state.fetch_or(DETACHED, Relaxed);

        if old & (DONE | RESULT_TAKEN) == DONE {
            // If the future is done, we should eagerly drop the result. This needs to be acquire
            // ordering because another thread might have written the result.

            // SAFETY: The future has completed.
            unsafe {
                meta.drop_result(Acquire);
            }
        }
    }

    /// Marks the task as aborted and detached (for when the caller isn't interested in waiting
    /// for the cancellation to be finished). Returns true if the task should be added to a run
    /// queue.
    pub(crate) fn abort_and_detach(&self) -> AbortAndDetachResult {
        let meta = self.meta();
        let old_state = meta.state.fetch_or(ABORTED | DETACHED | READY, Relaxed);
        if old_state & DONE != 0 {
            // If the future is done, we should eagerly drop the result. This needs to be acquire
            // ordering because another thread might have written the result.

            // SAFETY: The future has completed.
            unsafe {
                meta.drop_result(Acquire);
            }

            AbortAndDetachResult::Done
        } else if old_state & (INACTIVE | READY) == INACTIVE {
            AbortAndDetachResult::AddToRunQueue
        } else {
            AbortAndDetachResult::Pending
        }
    }

    /// Returns true if the task is detached.
    pub(crate) fn is_detached(&self) -> bool {
        self.meta().state.load(Relaxed) & DETACHED != 0
    }

    /// Returns true if the task is aborted.
    pub(crate) fn is_aborted(&self) -> bool {
        self.meta().state.load(Relaxed) & ABORTED != 0
    }

    /// Takes the result.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that `R` is the correct type.
    pub(crate) unsafe fn take_result<R>(&self) -> Option<R> {
        // This needs to be Acquire ordering to synchronize with the polling thread.
        let meta = self.meta();
        if meta.state.load(Relaxed) & (DONE | RESULT_TAKEN) == DONE
            && meta.state.fetch_or(RESULT_TAKEN, Acquire) & RESULT_TAKEN == 0
        {
            Some(unsafe { ((meta.vtable.get_result)(meta.into()) as *const R).read() })
        } else {
            None
        }
    }

    /// Marks the task as low priority.  Returns the old state.
    pub(crate) fn set_low_priority(&self, v: bool) -> bool {
        let prev = if v {
            self.meta().state.fetch_or(LOW_PRIORITY, Relaxed)
        } else {
            self.meta().state.fetch_and(!LOW_PRIORITY, Relaxed)
        };
        prev & LOW_PRIORITY != 0
    }

    /// Returns true if this is a low priority task.
    pub(crate) fn is_low_priority(&self) -> bool {
        self.meta().state.load(Relaxed) & LOW_PRIORITY != 0
    }
}

impl AtomicFutureHandle<'static> {
    /// Returns a waker for the future.
    pub(crate) fn waker(&self) -> BorrowedWaker<'_> {
        static BORROWED_WAKER_VTABLE: RawWakerVTable =
            RawWakerVTable::new(waker_clone, waker_wake_by_ref, waker_wake_by_ref, waker_noop);
        static WAKER_VTABLE: RawWakerVTable =
            RawWakerVTable::new(waker_clone, waker_wake, waker_wake_by_ref, waker_drop);

        fn waker_clone(raw_meta: *const ()) -> RawWaker {
            // SAFETY: We did the reverse cast below.
            let meta = unsafe { &*(raw_meta as *const Meta) };
            meta.retain();
            RawWaker::new(raw_meta, &WAKER_VTABLE)
        }

        fn waker_wake(raw_meta: *const ()) {
            // SAFETY: We did the reverse cast below.
            let meta = unsafe { &*(raw_meta as *const Meta) };
            if meta.state.fetch_or(READY, Relaxed) & (INACTIVE | READY | DONE) == INACTIVE {
                // This consumes the reference count.
                meta.scope().executor().task_is_ready(AtomicFutureHandle(
                    // SAFETY: We know raw_meta is not null.
                    unsafe { NonNull::new_unchecked(raw_meta as *mut Meta) },
                    PhantomData,
                ));
            } else {
                meta.release();
            }
        }

        fn waker_wake_by_ref(meta: *const ()) {
            // SAFETY: We did the reverse cast below.
            let meta = unsafe { &*(meta as *const Meta) };
            // SAFETY: The lifetime on `AtomicFutureHandle` is 'static.
            unsafe {
                meta.wake();
            }
        }

        fn waker_noop(_meta: *const ()) {}

        fn waker_drop(meta: *const ()) {
            // SAFETY: We did the reverse cast below.
            let meta = unsafe { &*(meta as *const Meta) };
            meta.release();
        }

        BorrowedWaker(
            // SAFETY: We meet the contract for RawWaker/RawWakerVtable.
            unsafe {
                Waker::from_raw(RawWaker::new(self.0.as_ptr() as *const (), &BORROWED_WAKER_VTABLE))
            },
            PhantomData,
        )
    }

    /// Wakes the future.
    pub(crate) fn wake(&self) {
        // SAFETY: The lifetime on `AtomicFutureHandle` is 'static.
        unsafe {
            self.meta().wake();
        }
    }

    /// Wakes the future with an active guard. Returns true if successful i.e. a guard needs to be
    /// acquired.
    ///
    /// NOTE: `Scope::release_cancel_guard` can be called *before* this function returns because the
    /// task can be polled on another thread. For this reason, the caller either needs to hold a
    /// lock, or it should preemptively take the guard.
    pub(crate) fn wake_with_active_guard(&self) -> bool {
        // SAFETY: The lifetime on `AtomicFutureHandle` is 'static.
        unsafe { self.meta().wake_with_active_guard() }
    }
}

impl<F: Future> Drop for AtomicFuture<F> {
    fn drop(&mut self) {
        let meta = &mut self.meta;
        // This needs to be acquire ordering so that we see writes that might have just happened
        // in another thread when the future was polled.
        let state = meta.state.load(Acquire);
        if state & DONE == 0 {
            // SAFETY: The state isn't DONE so we must drop the future.
            unsafe {
                (meta.vtable.drop_future)(meta.into());
            }
        } else if state & RESULT_TAKEN == 0 {
            // SAFETY: The result hasn't been taken so we must drop the result.
            unsafe {
                (meta.vtable.drop_result)(meta.into());
            }
        }
    }
}

pub struct BorrowedWaker<'a>(std::task::Waker, PhantomData<&'a ()>);

impl Deref for BorrowedWaker<'_> {
    type Target = Waker;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct Bomb;
impl Drop for Bomb {
    fn drop(&mut self) {
        std::process::abort();
    }
}
