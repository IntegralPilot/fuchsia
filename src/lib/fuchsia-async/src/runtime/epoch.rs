// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Epoch based deferred execution

use fuchsia_sync::{Mutex, MutexGuard};
use smallvec::SmallVec;
use std::collections::VecDeque;
use std::future::{Future, poll_fn};
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::sync::LazyLock;
use std::task::{Poll, RawWakerVTable, Waker};

/// Epoch implements epoch based deferred execution
#[derive(Default)]
pub struct Epoch {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    // Contains a list of deferred callbacks intermixed with special entries which hold the
    // reference counts for each epoch.  The first entry, if there is one, should always be a
    // reference count.  Once it reaches zero, callbacks that follow are called until we encounter
    // another non-zero reference count entry.
    callbacks: VecDeque<Callback>,

    // Every entry in the queue is given an increasing sequence number.  `sequence` here is the
    // sequence number assigned to the first callback in `callbacks`.
    sequence: usize,
}

// Callback is either a callback that has been added using `defer` or a guard count (if `vtable`
// is None).
struct Callback {
    data: Data,

    // We use RawWakerVTable so that we can easily make this work for wakers, but we only use the
    // `wake` and `drop` functions.
    vtable: Option<&'static RawWakerVTable>,
}

#[repr(C)]
union Data {
    // This is used if `vtable` is Some.
    data: *const (),

    // This is used as a reference count when `vtable` is None.
    count: usize,
}

// SAFETY: This is safe so long as the contract for RawWakerVTable is upheld.
unsafe impl Send for Data {}

/// An epoch guard. See `guard` below.
pub struct EpochGuard<'a> {
    epoch: &'a Epoch,

    // This is the sequence number of the entry in `callbacks` that has the count.
    sequence: usize,
}

/// A reference to the deferred callback.
pub struct CallbackRef<'a> {
    epoch: &'a Epoch,

    // This is the sequence number for the entry in `callbacks`.
    sequence: usize,
}

// PendingCallbacks must be dropped after the guard.
struct InnerGuard<'a>(MutexGuard<'a, Inner>, PendingCallbacks);

#[derive(Default)]
struct PendingCallbacks(SmallVec<[Callback; 4]>);

impl Epoch {
    /// Schedule `callback` to be executed when all prior references have been returned. If
    /// `callback` is no bigger than `usize`, typically no heap allocation will be incurred.  If
    /// there are no outstanding guards, `callback` will be called immediately.
    pub fn defer<F: FnOnce() + Send + Unpin + 'static>(&self, callback: F) -> CallbackRef<'_> {
        CallbackRef { epoch: self, sequence: self.inner_guard().defer(callback.into()) }
    }

    /// Same as `defer` but for a waker.
    pub fn defer_waker(&self, waker: &Waker) -> CallbackRef<'_> {
        CallbackRef { epoch: self, sequence: self.inner_guard().defer(waker.clone().into()) }
    }

    /// Takes a guard on the current epoch. Subsequent callbacks queued via `defer` are guaranteed
    /// not to be called before this guard is dropped.
    pub fn guard(&self) -> EpochGuard<'_> {
        EpochGuard { epoch: self, sequence: self.inner.lock().add_ref() }
    }

    /// Returns a reference to the global Epoch instance.
    pub fn global() -> &'static Epoch {
        static GLOBAL: LazyLock<Epoch> = LazyLock::new(Epoch::default);
        &GLOBAL
    }

    /// Waits for all previous guards to be released.  The barrier is initialised when the future is
    /// created, not when it is first polled.
    pub fn barrier(&self) -> impl Future<Output = ()> + '_ {
        let cb = self.defer_waker(Waker::noop());
        poll_fn(
            move |cx| {
                if cb.replace_waker(cx.waker()) { Poll::Pending } else { Poll::Ready(()) }
            },
        )
    }

    fn inner_guard(&self) -> InnerGuard<'_> {
        InnerGuard(self.inner.lock(), PendingCallbacks::default())
    }
}

impl Inner {
    // Add a reference to the current epoch.
    fn add_ref(&mut self) -> usize {
        if let Some(count) = self.callbacks.back_mut().and_then(|cb| cb.count_mut()) {
            *count += 1;
        } else {
            self.callbacks.push_back(Callback { data: Data { count: 1 }, vtable: None });
        }
        self.sequence + self.callbacks.len() - 1
    }
}

impl InnerGuard<'_> {
    fn defer(&mut self, callback: Callback) -> usize {
        let InnerGuard(inner, pending_callbacks) = self;
        if inner.callbacks.front().is_none_or(|cb| cb.count().unwrap() == 0) {
            // There are no outstanding guards, so call the callback immediately.
            pending_callbacks.push(callback);

            // Return a sequence of 0, which `has_fired` below will always return true for.
            0
        } else {
            inner.callbacks.push_back(callback);
            inner.sequence + inner.callbacks.len() - 1
        }
    }

    // Decrement a reference to the epoch at `sequence`.
    fn sub_ref(&mut self, sequence: usize) {
        let InnerGuard(inner, pending_callbacks) = self;
        let index = sequence - inner.sequence;
        let count = inner.callbacks[index].count_mut().unwrap();
        *count -= 1;
        // We need to call the callbacks if the count has reached zero, and the count is at the
        // beginning of `callbacks` *and* there are actually callbacks queued.
        if *count == 0 && index == 0 && inner.callbacks.len() > 1 {
            while let Some(callback) = inner.callbacks.front() {
                if let Some(count) = callback.count() {
                    if count > 0 || inner.callbacks.len() == 1 {
                        // We've encountered a count element which is either non-zero or has no
                        // callbacks after it, so we're done.
                        break;
                    }
                    inner.callbacks.pop_front();
                } else {
                    pending_callbacks.push(inner.callbacks.pop_front().unwrap());
                }
                inner.sequence += 1;
            }
        }
    }
}

impl PendingCallbacks {
    fn push(&mut self, callback: Callback) {
        self.0.push(callback);
    }
}

impl Drop for PendingCallbacks {
    fn drop(&mut self) {
        for callback in self.0.drain(..) {
            callback.call();
        }
    }
}

impl Callback {
    fn new(data: *const (), vtable: &'static RawWakerVTable) -> Self {
        Self { data: Data { data }, vtable: Some(vtable) }
    }

    fn count(&self) -> Option<usize> {
        if self.vtable.is_none() {
            // SAFETY: If vtable is None, then it must be a count.
            Some(unsafe { self.data.count })
        } else {
            None
        }
    }

    fn count_mut(&mut self) -> Option<&mut usize> {
        if self.vtable.is_none() {
            // SAFETY: If vtable is None, then it must be a count.
            Some(unsafe { &mut self.data.count })
        } else {
            None
        }
    }

    fn call(mut self) {
        // SAFETY: This is safe so long as the contract for RawWakerVTable is upheld.
        unsafe {
            Waker::new(self.data.data, self.vtable.take().unwrap()).wake();
        }
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        if let Some(vtable) = self.vtable {
            // SAFETY: Safe so long as the contract for RawWakerVTable is upheld.
            drop(unsafe { Waker::new(self.data.data, vtable) });
        }
    }
}

impl<F: FnOnce() + Send + Unpin + 'static> From<F> for Callback {
    fn from(value: F) -> Self {
        if std::mem::size_of::<F>() <= std::mem::size_of::<*const ()>() {
            struct InlineCallback<F>(PhantomData<F>);

            impl<F: FnOnce() + Send + Unpin + 'static> InlineCallback<F> {
                const VTABLE: RawWakerVTable = RawWakerVTable::new(
                    |_| unreachable!(),
                    Self::wake,
                    |_| unreachable!(),
                    Self::drop,
                );

                unsafe fn wake(data: *const ()) {
                    // SAFETY: We know `data` must be valid for size_of::<F>() bytes because we
                    // copied that many bytes below.
                    let callback = unsafe { std::mem::transmute_copy::<*const (), F>(&data) };
                    callback();
                }

                unsafe fn drop(data: *const ()) {
                    // SAFETY: We know `data` must be valid for size_of::<F>() bytes because we
                    // copied that many bytes below.
                    drop(unsafe { std::mem::transmute_copy::<*const (), F>(&data) });
                }
            }

            let mut data = std::ptr::null();
            let callback = ManuallyDrop::new(value);
            // SAFETY: We checked the size of `F` above.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    callback.deref() as *const F as *const u8,
                    &mut data as *mut _ as *mut u8,
                    std::mem::size_of::<F>(),
                );
            }
            Self::new(data, &InlineCallback::<F>::VTABLE)
        } else {
            struct BoxCallback<F>(PhantomData<F>);

            impl<F: FnOnce() + Send + Unpin + 'static> BoxCallback<F> {
                const VTABLE: RawWakerVTable = RawWakerVTable::new(
                    |_| unreachable!(),
                    Self::wake,
                    |_| unreachable!(),
                    Self::drop,
                );

                unsafe fn wake(data: *const ()) {
                    // SAFETY: This is just the reverse of what we do below.
                    let callback = unsafe { Box::from_raw(data as *mut F) };
                    callback();
                }

                unsafe fn drop(data: *const ()) {
                    // SAFETY: This is just the reverse of what we do below.
                    drop(unsafe { Box::from_raw(data as *mut F) });
                }
            }
            Callback::new(Box::into_raw(Box::new(value)) as *const (), &BoxCallback::<F>::VTABLE)
        }
    }
}

impl From<Waker> for Callback {
    fn from(waker: Waker) -> Self {
        // We consume the waker.
        let waker = ManuallyDrop::new(waker);
        Callback::new(waker.data(), waker.vtable())
    }
}

impl Clone for EpochGuard<'_> {
    fn clone(&self) -> Self {
        let mut inner = self.epoch.inner.lock();
        let index = self.sequence - inner.sequence;
        *inner.callbacks[index].count_mut().unwrap() += 1;
        Self { epoch: self.epoch, sequence: self.sequence }
    }
}

impl Drop for EpochGuard<'_> {
    fn drop(&mut self) {
        self.epoch.inner_guard().sub_ref(self.sequence);
    }
}

impl CallbackRef<'_> {
    /// Returns true if the callback has fired.
    pub fn has_fired(&self) -> bool {
        // We use <= because the first entry in callbacks should always be a reference count, and we
        // use sequence 0 when a callback is immediately called.
        self.sequence <= self.epoch.inner.lock().sequence
    }

    /// Replaces the callback with a different callback. Returns `true` if successful, or `false` if
    /// the existing callback has already been called.
    #[must_use]
    pub fn replace<F: FnOnce() + Send + Unpin + 'static>(&self, callback: F) -> bool {
        let mut inner = self.epoch.inner.lock();
        if self.sequence <= inner.sequence {
            return false;
        }
        let index = self.sequence - inner.sequence;
        inner.callbacks[index] = callback.into();
        true
    }

    /// Same as `replace` but for a waker.
    #[must_use]
    pub fn replace_waker(&self, waker: &Waker) -> bool {
        let mut inner = self.epoch.inner.lock();
        if self.sequence <= inner.sequence {
            return false;
        }
        let index = self.sequence - inner.sequence;
        inner.callbacks[index] = waker.clone().into();
        true
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use futures::stream::{FuturesUnordered, StreamExt};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::{iter, thread};

    #[test]
    fn test_defer() {
        let epoch = Epoch::default();
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();
        let guard = epoch.guard();
        let _cb = epoch.defer(move || called_clone.store(true, Ordering::Relaxed));
        assert!(!called.load(Ordering::Relaxed));
        drop(guard);
        assert!(called.load(Ordering::Relaxed));
    }

    #[test]
    fn test_defer_large_callback() {
        let epoch = Epoch::default();
        let large_data = [0u8; 1024];
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();
        let guard = epoch.guard();
        let _cb = epoch.defer(move || {
            assert_eq!(large_data.len(), 1024);
            called_clone.store(true, Ordering::Relaxed);
        });
        assert!(!called.load(Ordering::Relaxed));
        drop(guard);
        assert!(called.load(Ordering::Relaxed));
    }

    #[test]
    fn test_defer_small_callback() {
        let epoch = Epoch::default();
        epoch.defer(|| {});
        let b = 13u8;
        let callback = move || assert_eq!(b, 13);
        epoch.defer(callback);
    }

    #[test]
    fn test_defer_when_no_guards() {
        let epoch = Epoch::default();
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();
        let cb = epoch.defer(move || called_clone.store(true, Ordering::Relaxed));
        assert!(called.load(Ordering::Relaxed));
        assert!(cb.has_fired());
        assert!(!cb.replace(|| {}));
        assert!(!cb.replace_waker(Waker::noop()));
    }

    #[test]
    fn test_multiple_guards() {
        let epoch = Epoch::default();
        let guard1 = epoch.guard();
        let guard2 = epoch.guard();
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();
        let _cb = epoch.defer(move || called_clone.store(true, Ordering::Relaxed));
        assert!(!called.load(Ordering::Relaxed));
        drop(guard1);
        assert!(!called.load(Ordering::Relaxed));
        drop(guard2);
        assert!(called.load(Ordering::Relaxed));
    }

    #[test]
    fn test_multiple_guards_in_different_epoch() {
        let epoch = Epoch::default();
        let guard1 = epoch.guard();
        let called1 = Arc::new(AtomicBool::new(false));
        let called1_clone = called1.clone();
        let _cb = epoch.defer(move || called1_clone.store(true, Ordering::Relaxed));
        let guard2 = epoch.guard();
        let called2 = Arc::new(AtomicBool::new(false));
        let called2_clone = called2.clone();
        let _cb = epoch.defer(move || called2_clone.store(true, Ordering::Relaxed));
        assert!(!called1.load(Ordering::Relaxed));
        assert!(!called2.load(Ordering::Relaxed));
        drop(guard1);
        assert!(called1.load(Ordering::Relaxed));
        assert!(!called2.load(Ordering::Relaxed));
        drop(guard2);
        assert!(called2.load(Ordering::Relaxed));
    }

    #[test]
    fn test_multiple_guards_in_different_epoch_reverse_order() {
        let epoch = Epoch::default();
        let guard1 = epoch.guard();
        let called1 = Arc::new(AtomicBool::new(false));
        let called1_clone = called1.clone();
        let _cb = epoch.defer(move || called1_clone.store(true, Ordering::Relaxed));
        let guard2 = epoch.guard();
        let called2 = Arc::new(AtomicBool::new(false));
        let called2_clone = called2.clone();
        let _cb = epoch.defer(move || called2_clone.store(true, Ordering::Relaxed));
        assert!(!called1.load(Ordering::Relaxed));
        assert!(!called2.load(Ordering::Relaxed));
        // Drop guard2 first.
        drop(guard2);
        assert!(!called1.load(Ordering::Relaxed));
        assert!(!called2.load(Ordering::Relaxed));
        drop(guard1);
        assert!(called1.load(Ordering::Relaxed));
        assert!(called2.load(Ordering::Relaxed));
    }

    #[test]
    fn test_barrier() {
        let epoch = Epoch::default();
        let guard = epoch.guard();
        let barrier_future = epoch.barrier();
        // Use `FuturesUnordered because it uses its own wakers and so this will check that
        // the waker is actually called.
        let mut barrier_future: FuturesUnordered<_> = iter::once(barrier_future).collect();
        let mut cx = std::task::Context::from_waker(Waker::noop());
        assert!(barrier_future.poll_next_unpin(&mut cx).is_pending());
        drop(guard);
        assert!(barrier_future.poll_next_unpin(&mut cx).is_ready());
    }

    #[test]
    fn test_has_fired() {
        let epoch = Epoch::default();
        let guard = epoch.guard();
        let cb = epoch.defer(|| {});
        assert!(!cb.has_fired());
        drop(guard);
        assert!(cb.has_fired());
    }

    #[test]
    fn test_replace() {
        let epoch = Epoch::default();
        let called1 = Arc::new(AtomicBool::new(false));
        let called2 = Arc::new(AtomicBool::new(false));
        let called1_clone = called1.clone();
        let called2_clone = called2.clone();
        let guard = epoch.guard();
        let cb = epoch.defer(move || called1_clone.store(true, Ordering::Relaxed));
        assert!(cb.replace(move || called2_clone.store(true, Ordering::Relaxed)));
        drop(guard);
        assert!(!called1.load(Ordering::Relaxed));
        assert!(called2.load(Ordering::Relaxed));
        assert!(!cb.replace(|| {}));
    }

    #[test]
    fn test_barrier_race() {
        let epoch = Epoch::default();
        thread::scope(|s| {
            s.spawn(|| {
                for _ in 0..1000 {
                    let _guard = epoch.guard();
                }
            });
            s.spawn(|| {
                for _ in 0..1000 {
                    let _guard = epoch.guard();
                }
            });
            s.spawn(|| {
                for _ in 0..1000 {
                    futures::executor::block_on(async {
                        epoch.barrier().await;
                    });
                }
            });
        });
    }

    #[test]
    fn test_callback_reentrancy() {
        let epoch = Arc::new(Epoch::default());
        let counter = Arc::new(AtomicU8::new(0));

        let epoch_clone = epoch.clone();
        let counter_clone = counter.clone();
        epoch.defer(move || {
            epoch_clone.defer(move || {
                counter_clone.fetch_add(1, Ordering::Relaxed);
            });
        });

        assert_eq!(counter.load(Ordering::Relaxed), 1);

        let guard = epoch.guard();
        let epoch_clone = epoch.clone();
        let counter_clone = counter.clone();
        epoch.defer(move || {
            epoch_clone.defer(move || {
                counter_clone.fetch_add(1, Ordering::Relaxed);
            });
        });

        assert_eq!(counter.load(Ordering::Relaxed), 1);

        drop(guard);

        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }
}
