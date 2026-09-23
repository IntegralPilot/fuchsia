// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_rcu::RcuDroppable;

#[cfg(feature = "detect_lock_dep_cycles")]
mod tracking {
    use fuchsia_rcu::RcuDroppable;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_THREAD_ID: AtomicUsize = AtomicUsize::new(1);

    thread_local! {
        static THREAD_ID: usize = NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed);
    }

    fn current_thread_id() -> usize {
        THREAD_ID.with(|id| *id)
    }

    #[derive(RcuDroppable)]
    pub struct ThreadAffinityState {
        owner: AtomicUsize,
    }

    impl ThreadAffinityState {
        pub const fn new() -> Self {
            Self { owner: AtomicUsize::new(0) }
        }

        #[inline(always)]
        pub fn attach(&self) -> ThreadAffinityToken<'_> {
            let id = current_thread_id();
            let previous = self.owner.swap(id, Ordering::Relaxed);
            assert_eq!(previous, 0, "ThreadAffinity: Object is already attached to a thread!");
            ThreadAffinityToken { state: self }
        }

        #[inline(always)]
        pub fn assert_attached(&self) {
            let id = current_thread_id();
            assert_eq!(
                self.owner.load(Ordering::Relaxed),
                id,
                "ThreadAffinity: Object is not attached to the current thread!"
            );
        }

        #[inline(always)]
        pub fn assert_not_attached(&self) {
            let id = current_thread_id();
            assert_ne!(
                self.owner.load(Ordering::Relaxed),
                id,
                "ThreadAffinity: Object is already attached to the current thread!"
            );
        }
    }

    pub struct ThreadAffinityToken<'a> {
        state: &'a ThreadAffinityState,
    }

    impl<'a> Drop for ThreadAffinityToken<'a> {
        fn drop(&mut self) {
            self.state.owner.store(0, Ordering::Relaxed);
        }
    }
}

#[cfg(not(feature = "detect_lock_dep_cycles"))]
mod tracking {
    use fuchsia_rcu::RcuDroppable;

    #[derive(RcuDroppable)]
    pub struct ThreadAffinityState {}

    impl ThreadAffinityState {
        #[inline(always)]
        pub const fn new() -> Self {
            Self {}
        }

        #[inline(always)]
        pub fn attach(&self) -> ThreadAffinityToken<'_> {
            ThreadAffinityToken { _state: self }
        }

        #[inline(always)]
        pub fn assert_attached(&self) {}

        #[inline(always)]
        pub fn assert_not_attached(&self) {}
    }

    pub struct ThreadAffinityToken<'a> {
        _state: &'a ThreadAffinityState,
    }
}

/// A synchronization primitive that tracks thread affinity.
///
/// It provides mechanisms to attach an object to a thread and assert that
/// the object is or is not attached to the current thread. When the feature
/// `detect_lock_dep_cycles` is disabled, this struct and its methods are zero-cost.
#[derive(RcuDroppable)]
pub struct ThreadAffinity {
    state: tracking::ThreadAffinityState,
}

impl ThreadAffinity {
    pub const fn new() -> Self {
        Self { state: tracking::ThreadAffinityState::new() }
    }

    /// Attaches this object to the current thread.
    ///
    /// Returns a `ThreadAffinityGuard` that will detach the object when dropped.
    #[inline(always)]
    pub fn attach(&self) -> ThreadAffinityGuard<'_> {
        ThreadAffinityGuard { _token: self.state.attach() }
    }

    /// Asserts that this object is attached to the current thread.
    ///
    /// Panics if it is not attached.
    #[inline(always)]
    pub fn assert_attached(&self) {
        self.state.assert_attached();
    }

    /// Asserts that this object is NOT attached to the current thread.
    ///
    /// Panics if it is attached.
    #[inline(always)]
    pub fn assert_not_attached(&self) {
        self.state.assert_not_attached();
    }
}

impl Default for ThreadAffinity {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ThreadAffinityGuard<'a> {
    _token: tracking::ThreadAffinityToken<'a>,
}
