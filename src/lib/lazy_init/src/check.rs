// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://github.com/rust-lang/rust/issues/143874): Replace constants
// with the return values of a const trait method instead.
#![allow(clippy::declare_interior_mutable_const)]

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU8, Ordering};

/// Lifecycle states of a [`crate::LazyInit`]-wrapped object.
#[derive(Clone, Copy, Eq, Debug, PartialEq)]
#[repr(u8)]
pub enum Lifecycle {
    Uninitialized = 0,
    Initializing = 1,
    Initialized = 2,
}

impl Lifecycle {
    pub const fn from_u8(val: u8) -> Self {
        match val {
            0 => Lifecycle::Uninitialized,
            1 => Lifecycle::Initializing,
            2 => Lifecycle::Initialized,
            _ => unreachable!(),
        }
    }
}

/// Lifecycle and storage policy for initialization state.
pub trait Policy {
    /// Associated storage for state tracking.
    type State;

    /// An instance of state in the uninitialized condition.
    const UNINIT_STATE: Self::State;

    /// Transitions `state` from `from` to `to`, panicking if the current state
    /// does not match `from`.
    fn transition(state: &Self::State, from: Lifecycle, to: Lifecycle);

    /// Executes `init_fn` to construct the wrapped value within an
    /// initialization transition.
    ///
    /// Transitions to the initialized state if `init_fn` succeeds with `Ok`, or
    /// reverts to the uninitialized state if it returns `Err`.
    ///
    /// # Panics
    ///
    /// Panics if the instance is not currently uninitialized.
    #[inline]
    fn init_with<R, E>(
        state: &Self::State,
        init_fn: impl FnOnce() -> Result<R, E>,
    ) -> Result<R, E> {
        Self::transition(state, Lifecycle::Uninitialized, Lifecycle::Initializing);
        let result = init_fn();
        let end_state =
            if result.is_ok() { Lifecycle::Initialized } else { Lifecycle::Uninitialized };
        Self::transition(state, Lifecycle::Initializing, end_state);
        result
    }
}

/// An initialization policy that actively validates initialization state.
pub trait CheckedPolicy: Policy {
    /// Asserts that the instance has been initialized.
    ///
    /// This method is called on the hot path by [`crate::LazyInit::get`] before
    /// reading the wrapped value. Implementations should always inline and
    /// reduce to a single load, compare, and conditional branch to an
    /// out-of-line, cold panic path.
    ///
    /// # Panics
    ///
    /// Panics if the instance has not been initialized.
    fn assert_initialized(state: &Self::State);
}

// Out-of-line cold panic path per `assert_initialized()`.
#[cold]
#[inline(never)]
fn panic_uninitialized() -> ! {
    panic!("LazyInit: accessed before initialization");
}

fn panic_unexpected_state(actual: Lifecycle) -> ! {
    match actual {
        Lifecycle::Initializing => {
            panic!("LazyInit: concurrent or reentrant initialization in progress")
        }
        Lifecycle::Initialized => panic!("LazyInit: already initialized"),
        _ => panic!("LazyInit: invalid state transition"),
    }
}

pub struct NoCheck;

impl Policy for NoCheck {
    type State = ();

    const UNINIT_STATE: Self::State = ();

    fn transition(_state: &Self::State, _from: Lifecycle, _to: Lifecycle) {}
}

/// Check strategy that defers synchronization to the caller.
pub struct BasicCheck;

impl Policy for BasicCheck {
    type State = UnsafeCell<Lifecycle>;

    const UNINIT_STATE: Self::State = UnsafeCell::new(Lifecycle::Uninitialized);

    fn transition(state: &Self::State, from: Lifecycle, to: Lifecycle) {
        // Safety: synchronization is deferred to the caller.
        let current = unsafe { *state.get() };
        if current != from {
            panic_unexpected_state(current);
        }
        // Safety: synchronization is deferred to the caller.
        unsafe { state.get().write(to) };
    }
}

impl CheckedPolicy for BasicCheck {
    // Implemented per the documented criteria of the trait method.
    #[inline(always)]
    fn assert_initialized(state: &Self::State) {
        // Safety: synchronization is deferred to the caller.
        if unsafe { *state.get() } != Lifecycle::Initialized {
            panic_uninitialized();
        }
    }
}

/// Check strategy that accesses and updates state atomically.
pub struct AtomicCheck;

impl Policy for AtomicCheck {
    type State = AtomicU8;

    const UNINIT_STATE: Self::State = AtomicU8::new(Lifecycle::Uninitialized as u8);

    fn transition(state: &Self::State, from: Lifecycle, to: Lifecycle) {
        let order = match to {
            // Acquire: Prevents memory writes during initialization from being
            // reordered before claiming "initializing" exclusivity, and also
            // synchronizes with any prior failed initialization that reverted
            // state back to uninitialized.
            Lifecycle::Initializing => Ordering::Acquire,

            // Release: Publishes all memory writes performed during
            // initialization so that readers observing `Initialized` with
            // `Acquire` see the fully constructed value.
            Lifecycle::Initialized => Ordering::Release,

            // Release: Ensures any cleanup or drop writes from a failed
            // initialization are committed before resetting state to
            // `Uninitialized`.
            Lifecycle::Uninitialized => Ordering::Release,
        };
        if let Err(actual) = state.compare_exchange(from as u8, to as u8, order, Ordering::Relaxed)
        {
            panic_unexpected_state(Lifecycle::from_u8(actual));
        }
    }
}

impl CheckedPolicy for AtomicCheck {
    // Implemented per the documented criteria of the trait method.
    #[inline(always)]
    fn assert_initialized(state: &Self::State) {
        // Acquire: Synchronizes with the `Release` write when transitioning to
        // `Initialized`, ensuring all writes constructing the wrapped value
        // are visible before this thread reads the payload.
        if state.load(Ordering::Acquire) != Lifecycle::Initialized as u8 {
            panic_uninitialized();
        }
    }
}
