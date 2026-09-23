// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Types and traits providing send buffer management for netstack sockets.

use netstack3_sync::Mutex;
use netstack3_sync::atomic::{AtomicIsize, Ordering};

use crate::num::PositiveIsize;

/// Tracks the available send buffer space for a socket.
#[derive(Debug)]
pub struct SendBufferTracking<L> {
    /// Keeps track of how many bytes are available in the send buffer.
    ///
    /// Due to overcommit, this is allowed to become negative. Hence, whenever
    /// `available_bytes changes`, the only thing to be observed in terms of
    /// signaling the listener is whether the zero is crossed (from negative to
    /// positive and vice-versa) as part of adding or subtracting in flight
    /// bytes.
    available_bytes: AtomicIsize,
    inner: Mutex<Inner<L>>,
}

impl<L> SendBufferTracking<L> {
    /// Creates a new `SendBufferTracking` with the initial available send
    /// buffer space `capacity` and a writable listener `listener`.
    pub fn new(capacity: PositiveIsize, listener: L) -> Self {
        Self {
            available_bytes: AtomicIsize::new(capacity.into()),
            inner: Mutex::new(Inner {
                listener,
                // Listeners must assume the socket is writable from creation.
                // See SocketWritableListener.
                notified_state: true,
                capacity,
            }),
        }
    }

    /// Calls the callback `f` with a mutable reference to the listener.
    #[cfg(any(test, feature = "testutils"))]
    pub fn with_listener<R, F: FnOnce(&mut L) -> R>(&self, f: F) -> R {
        f(&mut self.inner.lock().listener)
    }
}

impl<L: SocketWritableListener> SendBufferTracking<L> {
    /// Acquires `bytes` for send with this `SendBufferTracking` instance.
    pub fn acquire(&self, bytes: PositiveIsize) -> Result<SendBufferSpace, SendBufferFullError> {
        let bytes_isize = bytes.into();
        // Ordering: Paired with Release in return_and_notify.
        let prev = self.available_bytes.fetch_sub(bytes_isize, Ordering::Acquire);
        // If we had enough bytes available allow the write.
        if prev > bytes_isize {
            return Ok(SendBufferSpace(bytes));
        }
        // Turns out we didn't have enough space. Place the bytes back in and
        // potentially notify, but return an error.
        //
        // The regular return flow here is necessary because we could be racing
        // with other agents acquiring and returning space, and this return
        // could be the one that flips the socket back into writable.
        if prev <= 0 {
            self.return_and_notify(bytes_isize);
            return Err(SendBufferFullError);
        }

        // prev is in the interval (0, bytes] meaning this allocation crossed a
        // threshold. Notify the listener before returning.
        self.notify();
        Ok(SendBufferSpace(bytes))
    }

    /// Releases `space` back to this `SendBufferTracking` instance.
    pub fn release(&self, space: SendBufferSpace) {
        let SendBufferSpace(delta) = &space;
        self.return_and_notify((*delta).into());
        // Prevent drop panic for send buffer space.
        core::mem::forget(space);
    }

    fn return_and_notify(&self, delta: isize) {
        // Ordering: Paired with Acquire in acquire.
        let prev = self.available_bytes.fetch_add(delta, Ordering::Release);
        if prev <= 0 && prev + delta > 0 {
            self.notify();
        }
    }

    fn notify(&self) {
        let Self { available_bytes, inner } = self;
        let mut inner = inner.lock();
        Self::notify_locked(available_bytes, &mut inner);
    }

    fn notify_locked(available_bytes: &AtomicIsize, inner: &mut Inner<L>) {
        let Inner { listener, notified_state, capacity: _ } = inner;
        // Read the available buffer space under lock and change the
        // notification state accordingly. Relaxed ordering is okay here because
        // the lock is guaranteeing the ordering.
        let new_writable = available_bytes.load(Ordering::Relaxed) > 0;
        if core::mem::replace(notified_state, new_writable) != new_writable {
            listener.on_writable_changed(new_writable);
        }
    }

    /// Returns the tracker's capacity.
    pub fn capacity(&self) -> PositiveIsize {
        self.inner.lock().capacity
    }

    /// Returns the currently available buffer space.
    pub fn available(&self) -> Option<PositiveIsize> {
        PositiveIsize::new(self.available_bytes.load(Ordering::Relaxed))
    }

    /// Updates the tracker's capacity to `new_capacity`.
    ///
    /// Note that upon changing the capacity the socket's writable state may
    /// change.
    pub fn set_capacity(&self, new_capacity: PositiveIsize) {
        let Self { available_bytes, inner } = self;
        let mut inner = inner.lock();
        let Inner { listener: _, notified_state: _, capacity } = &mut *inner;
        let old = core::mem::replace(capacity, new_capacity);
        let delta = new_capacity.get() - old.get();
        // Ordering: We're already under lock here and we want to ensure we're
        // not reordering with the relaxed load in notify_locked.
        let _: isize = available_bytes.fetch_add(delta, Ordering::AcqRel);
        // We already have the lock, check if we need to notify regardless of
        // whether we crossed zero or not.
        Self::notify_locked(available_bytes, &mut inner);
    }
}

#[derive(Debug)]
struct Inner<L> {
    listener: L,
    /// Keeps track of the expected "writable" state seen by `listener`.
    ///
    /// This is necessary because of the zero-crossing optimization implemented
    /// by [`SendBufferTracking`]. Multiple threads can be acquiring and
    /// releasing bytes from the buffer at the same time and the zero crossing
    /// itself doesn't fully guarantee that we'd never notify the listener twice
    /// with the same value. This boolean provides that guarantee.
    ///
    /// Example race:
    ///
    /// - T1 acquires N bytes observes zero-crossing and tries to acquire the
    ///   lock; gets descheduled.
    /// - T2 releases N bytes, observes zero crossing and tries to acquire the
    ///   lock; gets descheduled.
    /// - T3 acquires N bytes, observes zero crossing and tries to acquire the
    ///   lock; gets descheduled.
    ///
    /// Whenever the 3 threads become runnable again they'll acquire the lock
    /// one at a time, read the number of available bytes in the buffer and
    /// decide on a writable state, which would be attempted to be notified 3
    /// times in a row.
    notified_state: bool,
    /// The maximum capacity allowed in the [`SendBufferTracking`] instance.
    ///
    /// Note that, due to overcommit, more than `capacity` bytes can be
    /// in-flight at once. A single [`SendBufferSpace`] may be emitted that
    /// crosses the capacity threshold.
    capacity: PositiveIsize,
}

/// A type stating that some amount of space was reserved within a
/// [`SendBufferTracking`] instance.
///
/// This type is returned from [`SendBufferTracking::acquire`] and *must* be
/// returned to [`SendBufferTracking::release`] when the annotated send buffer
/// space is freed. Otherwise, [`SendBufferSpace::acknowledge_drop`] must be
/// called. This type panics on drop otherwise.
#[derive(Debug, Eq, PartialEq)]
pub struct SendBufferSpace(PositiveIsize);

impl Drop for SendBufferSpace {
    fn drop(&mut self) {
        panic!("dropped send buffer space with {:?} bytes", self)
    }
}

impl SendBufferSpace {
    /// Acknowledges an unused and from this point on untracked buffer space.
    ///
    /// This may be used to drop previously allocated buffer space tracking that
    /// has outlived the owning socket.
    pub fn acknowledge_drop(self) {
        core::mem::forget(self)
    }
}

/// An error indicating that the send buffer is full.
#[derive(Debug, Eq, PartialEq)]
pub struct SendBufferFullError;

/// A type capable of handling socket writable changes.
///
/// Upon creation, listeners must always assume the socket is writable.
pub trait SocketWritableListener {
    /// Notifies the listener the writable state has changed to `writable`.
    ///
    /// Callers must only call this when the writable state has actually
    /// changed. Implementers may panic if they see the current state being
    /// notified as changed.
    fn on_writable_changed(&mut self, writable: bool);
}

#[cfg(any(test, feature = "testutils"))]
pub(crate) mod testutil {
    use super::*;

    /// A fake [`SocketWritableListener`] implementation.
    #[derive(Debug)]
    pub struct FakeSocketWritableListener {
        writable: bool,
    }

    impl FakeSocketWritableListener {
        /// Returns whether the listener has observed a writable state.
        pub fn is_writable(&self) -> bool {
            self.writable
        }
    }

    impl Default for FakeSocketWritableListener {
        fn default() -> Self {
            Self { writable: true }
        }
    }

    impl SocketWritableListener for FakeSocketWritableListener {
        fn on_writable_changed(&mut self, writable: bool) {
            assert_ne!(core::mem::replace(&mut self.writable, writable), writable);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use testutil::FakeSocketWritableListener;

    use alloc::vec::Vec;

    const SNDBUF: PositiveIsize = PositiveIsize::new(4).unwrap();
    const HALF_SNDBUF: PositiveIsize = PositiveIsize::new(SNDBUF.get() / 2).unwrap();
    const TWO_SNDBUF: PositiveIsize = PositiveIsize::new(SNDBUF.get() * 2).unwrap();
    const ONE: PositiveIsize = PositiveIsize::new(1).unwrap();

    impl SendBufferTracking<FakeSocketWritableListener> {
        fn listener_writable(&self) -> bool {
            self.inner.lock().listener.is_writable()
        }
    }

    #[test]
    fn acquire_all_buffer() {
        let tracking = SendBufferTracking::new(SNDBUF, FakeSocketWritableListener::default());
        let acquired = tracking.acquire(SNDBUF).expect("acquire");
        assert_eq!(tracking.available_bytes.load(Ordering::SeqCst), 0);
        assert_eq!(tracking.listener_writable(), false);
        assert_eq!(tracking.acquire(ONE), Err(SendBufferFullError));
        tracking.release(acquired);
        assert_eq!(tracking.listener_writable(), true);
        assert_eq!(tracking.available_bytes.load(Ordering::SeqCst), SNDBUF.get());
    }

    #[test]
    fn acquire_half_buffer() {
        let tracking = SendBufferTracking::new(SNDBUF, FakeSocketWritableListener::default());
        let acquired = tracking.acquire(HALF_SNDBUF).expect("acquire");
        assert_eq!(
            tracking.available_bytes.load(Ordering::SeqCst),
            SNDBUF.get() - HALF_SNDBUF.get()
        );
        assert_eq!(tracking.listener_writable(), true);
        tracking.release(acquired);
        assert_eq!(tracking.listener_writable(), true);
        assert_eq!(tracking.available_bytes.load(Ordering::SeqCst), SNDBUF.get());
    }

    #[test]
    fn acquire_multiple() {
        let tracking = SendBufferTracking::new(SNDBUF, FakeSocketWritableListener::default());
        let tokens = (0..SNDBUF.get())
            .map(|_| {
                assert_eq!(tracking.listener_writable(), true);
                tracking.acquire(ONE).expect("acquire")
            })
            .collect::<Vec<_>>();
        assert_eq!(tracking.available_bytes.load(Ordering::SeqCst), 0);
        assert_eq!(tracking.listener_writable(), false);

        assert_eq!(tracking.acquire(ONE), Err(SendBufferFullError));
        for t in tokens {
            tracking.release(t);
            assert_eq!(tracking.listener_writable(), true);
        }

        assert_eq!(tracking.available_bytes.load(Ordering::SeqCst), SNDBUF.get());
        assert_eq!(tracking.listener_writable(), true);
    }

    #[test]
    fn overcommit_single_buffer() {
        let tracking = SendBufferTracking::new(SNDBUF, FakeSocketWritableListener::default());
        let acquired = tracking.acquire(TWO_SNDBUF).expect("acquire");
        assert_eq!(tracking.listener_writable(), false);
        assert_eq!(
            tracking.available_bytes.load(Ordering::SeqCst),
            SNDBUF.get() - TWO_SNDBUF.get()
        );
        assert_eq!(tracking.acquire(ONE), Err(SendBufferFullError));

        tracking.release(acquired);
        assert_eq!(tracking.listener_writable(), true);
        assert_eq!(tracking.available_bytes.load(Ordering::SeqCst), SNDBUF.get());
    }

    #[test]
    fn overcommit_two_buffers() {
        let tracking = SendBufferTracking::new(SNDBUF, FakeSocketWritableListener::default());
        let acquired1 = tracking.acquire(ONE).expect("acquire");
        assert_eq!(tracking.listener_writable(), true);
        assert_eq!(tracking.available_bytes.load(Ordering::SeqCst), SNDBUF.get() - 1);
        let acquired2 = tracking.acquire(SNDBUF).expect("acquire");
        assert_eq!(tracking.listener_writable(), false);
        assert_eq!(tracking.available_bytes.load(Ordering::SeqCst), -1);
        assert_eq!(tracking.acquire(ONE), Err(SendBufferFullError));

        tracking.release(acquired1);
        // Still not writable.
        assert_eq!(tracking.listener_writable(), false);
        assert_eq!(tracking.available_bytes.load(Ordering::SeqCst), 0);

        tracking.release(acquired2);
        assert_eq!(tracking.listener_writable(), true);
        assert_eq!(tracking.available_bytes.load(Ordering::SeqCst), SNDBUF.get());
    }

    #[test]
    fn capacity_increase_makes_writable() {
        let tracking = SendBufferTracking::new(SNDBUF, FakeSocketWritableListener::default());
        assert_eq!(tracking.capacity(), SNDBUF);
        let acquired = tracking.acquire(SNDBUF).expect("acquire");
        assert_eq!(tracking.listener_writable(), false);

        tracking.set_capacity(TWO_SNDBUF);
        assert_eq!(tracking.listener_writable(), true);
        assert_eq!(
            tracking.available_bytes.load(Ordering::SeqCst),
            TWO_SNDBUF.get() - SNDBUF.get()
        );
        assert_eq!(tracking.capacity(), TWO_SNDBUF);
        tracking.release(acquired);
    }

    #[test]
    fn capacity_decrease_makes_non_writable() {
        let tracking = SendBufferTracking::new(SNDBUF, FakeSocketWritableListener::default());
        assert_eq!(tracking.capacity(), SNDBUF);
        let acquired = tracking.acquire(HALF_SNDBUF).expect("acquire");
        assert_eq!(tracking.listener_writable(), true);

        tracking.set_capacity(HALF_SNDBUF);
        assert_eq!(tracking.listener_writable(), false);
        assert_eq!(tracking.available_bytes.load(Ordering::SeqCst), 0);
        assert_eq!(tracking.capacity(), HALF_SNDBUF);
        tracking.release(acquired);
        assert_eq!(tracking.listener_writable(), true);
        assert_eq!(tracking.available_bytes.load(Ordering::SeqCst), HALF_SNDBUF.get());
    }
}
