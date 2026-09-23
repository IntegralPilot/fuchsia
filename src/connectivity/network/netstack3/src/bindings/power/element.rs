// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Netstack3-internal power elements.

use std::hash::Hash;
use std::sync::Arc;
use std::sync::atomic::{self, AtomicUsize};
use std::task::Poll;

use futures::task::AtomicWaker;

use crate::bindings::power::BinaryPowerLevel;

/// A simple netstack-internal lessor.
///
/// A lessor can be used to take leases to [`InternalPowerElement`].
#[derive(Clone)]
pub(crate) struct InternalLessor(Arc<InternalPowerElementState>);

impl InternalLessor {
    /// Creates a new lease for this power element.
    pub(crate) fn lease(&self) -> InternalLease {
        let Self(state) = self;
        let prev = state.leases.fetch_add(1, atomic::Ordering::Relaxed);
        if prev == 0 {
            state.waker.wake();
        }
        InternalLease(state.clone())
    }
}

// Implement pointer equality so we can have a HashMap keyed on the power
// element.
impl PartialEq for InternalLessor {
    fn eq(&self, other: &Self) -> bool {
        let Self(this) = self;
        let Self(other) = other;
        Arc::ptr_eq(this, other)
    }
}

impl Eq for InternalLessor {}

// Implement pointer hashing so we can have a HashMap keyed on the power
// element.
impl Hash for InternalLessor {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let Self(this) = self;
        std::ptr::hash(Arc::as_ptr(this), state);
    }
}

/// A simple netstack-internal power element.
///
/// The internal power element is a short cut for only having On-Off states and
/// it can vend [`InternalLessor`] for taking leases on the element.
pub(crate) struct InternalPowerElement {
    shared: Arc<InternalPowerElementState>,
    current_level: BinaryPowerLevel,
}

impl InternalPowerElement {
    /// Creates a new [`InternalPowerElement`].
    ///
    /// The initial level of a power element is always
    /// [`BinaryPowerLevel::Off`]. The first call to
    /// [`InternalPowerElement::wait_level_change`] pends until the element is
    /// on.
    pub(crate) fn new() -> Self {
        Self {
            shared: Arc::new(InternalPowerElementState {
                leases: AtomicUsize::new(0),
                waker: AtomicWaker::default(),
            }),
            current_level: BinaryPowerLevel::Off,
        }
    }

    /// Retrieves a new lessor for this [`InternalPowerElement`].
    pub(crate) fn lessor(&self) -> InternalLessor {
        InternalLessor(self.shared.clone())
    }

    /// Returns the currently cached power level.
    ///
    /// This level only changes after being yielded by
    /// [`InternalPowerElement::wait_level_change`].
    pub(crate) fn current_level(&self) -> BinaryPowerLevel {
        self.current_level
    }

    /// Waits for the level to change from
    /// [`InternalPowerElement::current_level`], returning the new level.
    ///
    /// This function effectively drives lease observation. The value returned
    /// from [`InternalPowerElement::current_level`] only changes after it's
    /// been observed as a return from `wait_level_change`.
    pub(crate) async fn wait_level_change(&mut self) -> BinaryPowerLevel {
        let Self { shared, current_level } = self;
        let target = current_level.flip();
        let InternalPowerElementState { leases, waker } = &**shared;

        let mut poll_leases = || {
            let has_leases = leases.load(atomic::Ordering::Relaxed) != 0;
            match (target, has_leases) {
                (BinaryPowerLevel::On, true) | (BinaryPowerLevel::Off, false) => {
                    *current_level = target;
                    Some(target)
                }
                (BinaryPowerLevel::Off, true) | (BinaryPowerLevel::On, false) => None,
            }
        };

        futures::future::poll_fn(|ctx| {
            if let Some(level) = poll_leases() {
                return Poll::Ready(level);
            }
            waker.register(ctx.waker());
            if let Some(level) = poll_leases() {
                return Poll::Ready(level);
            }
            Poll::Pending
        })
        .await
    }
}

struct InternalPowerElementState {
    leases: AtomicUsize,
    waker: AtomicWaker,
}

/// A simple netstack-internal lease.
///
/// Represents an RAII lease to an [`InternalPowerElement`].
pub(crate) struct InternalLease(Arc<InternalPowerElementState>);

impl Drop for InternalLease {
    fn drop(&mut self) {
        let Self(state) = self;
        let prev = state.leases.fetch_sub(1, atomic::Ordering::Relaxed);
        assert_ne!(prev, 0);
        if prev == 1 {
            state.waker.wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::pin::pin;

    use fuchsia_async as fasync;

    #[test]
    fn lease_power_element() {
        let mut exec = fasync::TestExecutor::new();
        let mut pe = InternalPowerElement::new();
        assert_eq!(pe.current_level(), BinaryPowerLevel::Off);
        let lessor = pe.lessor();
        let lease1 = {
            let mut wait_change = pin!(pe.wait_level_change());
            assert_eq!(exec.run_until_stalled(&mut wait_change), Poll::Pending);

            let lease1 = lessor.lease();
            assert_eq!(exec.run_until_stalled(&mut wait_change), Poll::Ready(BinaryPowerLevel::On));
            lease1
        };
        assert_eq!(pe.current_level(), BinaryPowerLevel::On);

        {
            let mut wait_change = pin!(pe.wait_level_change());
            assert_eq!(exec.run_until_stalled(&mut wait_change), Poll::Pending);
            let lease2 = lessor.lease();
            assert_eq!(exec.run_until_stalled(&mut wait_change), Poll::Pending);
            drop(lease1);
            assert_eq!(exec.run_until_stalled(&mut wait_change), Poll::Pending);
            drop(lease2);
            assert_eq!(
                exec.run_until_stalled(&mut wait_change),
                Poll::Ready(BinaryPowerLevel::Off)
            );
        }
        assert_eq!(pe.current_level(), BinaryPowerLevel::Off);
    }
}
