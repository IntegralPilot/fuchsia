// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod client;

use std::fmt::Debug;
use std::time::Duration;

/// A type representing an instant in time.
pub trait Instant: Sized + Ord + Copy + Clone + Debug + Send + Sync + 'static {
    // Note that this trait intentionally does not have a `now` method to return
    // the current time instant to force the platform to provide the time instant
    // when an event is handled.

    /// Returns the amount of time elapsed from another instant to this one.
    ///
    /// # Panics
    ///
    /// This function will panic if `earlier` is later than `self`.
    fn duration_since(&self, earlier: Self) -> Duration;

    /// Returns `Some(t)` where `t` is the time `self + duration` if `t` can be
    /// represented as `Instant` (which means it's inside the bounds of the
    /// underlying data structure), `None` otherwise.
    fn checked_add(&self, duration: Duration) -> Option<Self>;
}

pub(crate) trait InstantExt: Instant {
    /// Adds the duration to the instant.
    ///
    /// We can't do a blanket `impl<I: Instant> Add<Duration> for I` because `I`
    /// may be foreign type and `Add` is also a foreign type. We use this ext
    /// trait to avoid having to do `.checked_add(..).unwrap()` when we want to
    /// add an [`Instant`] with a [`Duration`].
    ///
    /// # Panics
    ///
    /// This function may panic if the resulting point in time cannot be represented by the
    /// underlying data structure. See [`Instant::checked_add`] for a version without panic.
    fn add(self, duration: Duration) -> Self;
}

impl<I: Instant> InstantExt for I {
    fn add(self, duration: Duration) -> Self {
        self.checked_add(duration).unwrap_or_else(|| {
            panic!("overflow when adding duration={:?} to instant={:?}", duration, self,)
        })
    }
}

impl Instant for std::time::Instant {
    fn duration_since(&self, earlier: Self) -> Duration {
        std::time::Instant::duration_since(self, earlier)
    }

    fn checked_add(&self, duration: Duration) -> Option<Self> {
        std::time::Instant::checked_add(self, duration)
    }
}

// The client currently supports DUID-LL and DUID-LLT with MAC addresses only
// and DUID-UUID, the largest of which is DUID-UUID which takes up 18 bytes.
pub type ClientDuid = arrayvec::ArrayVec<u8, 18>;
