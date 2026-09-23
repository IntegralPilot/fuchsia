// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::TryFromIntError;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use fuchsia_async as fasync;
use netstack3_core::{AtomicInstant, Instant};

use crate::bindings::util::IntoFidl;
use crate::bindings::{InspectableValue, Inspector};

/// A thin wrapper around `fuchsia_async::MonotonicInstant` that implements `core::Instant`.
#[derive(PartialEq, Eq, PartialOrd, Ord, Copy, Clone, Debug)]
pub(crate) struct StackTime(fasync::MonotonicInstant);

impl StackTime {
    /// Construct a new [`StackTime`] from the given [`fasync::MonotonicInstant`].
    pub(crate) fn new(time: fasync::MonotonicInstant) -> StackTime {
        StackTime(time)
    }

    /// Construct a new [`StackTime`] at the current time.
    pub(crate) fn now() -> StackTime {
        StackTime(fasync::MonotonicInstant::now())
    }

    /// Construct a new [`StackTime`] from a [`zx::MonotonicInstant`].
    pub(crate) fn from_zx(time: zx::MonotonicInstant) -> StackTime {
        StackTime(fasync::MonotonicInstant::from_zx(time))
    }

    /// Convert [`StackTime`] into a [`zx::MonotonicInstant`].
    pub(crate) fn into_zx(self) -> zx::MonotonicInstant {
        let StackTime(time) = self;
        time.into_zx()
    }

    /// Convert [`StackTime`] into a [`fasync::MonotonicInstant`]
    pub(crate) fn into_fuchsia_time(self) -> fasync::MonotonicInstant {
        let StackTime(time) = self;
        time
    }
}

impl Instant for StackTime {
    fn checked_duration_since(&self, earlier: StackTime) -> Option<Duration> {
        match u64::try_from(self.0.into_nanos() - earlier.0.into_nanos()) {
            Ok(nanos) => Some(Duration::from_nanos(nanos)),
            Err(TryFromIntError { .. }) => None,
        }
    }

    fn checked_add(&self, duration: Duration) -> Option<StackTime> {
        Some(StackTime(fasync::MonotonicInstant::from_nanos(
            self.0.into_nanos().checked_add(i64::try_from(duration.as_nanos()).ok()?)?,
        )))
    }

    fn saturating_add(&self, duration: Duration) -> Self {
        StackTime(fasync::MonotonicInstant::from_nanos(
            self.0
                .into_nanos()
                .saturating_add(i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX)),
        ))
    }

    fn checked_sub(&self, duration: Duration) -> Option<StackTime> {
        Some(StackTime(fasync::MonotonicInstant::from_nanos(
            self.0.into_nanos().checked_sub(i64::try_from(duration.as_nanos()).ok()?)?,
        )))
    }
}

impl std::fmt::Display for StackTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self(time) = *self;
        write!(f, "{:.6}", time.into_nanos() as f64 / 1_000_000_000f64)
    }
}

impl InspectableValue for StackTime {
    fn record<I: Inspector>(&self, name: &str, inspector: &mut I) {
        let Self(inner) = self;
        inspector.record_int(name, inner.into_nanos())
    }
}

impl IntoFidl<i64> for StackTime {
    fn into_fidl(self) -> i64 {
        self.into_zx().into_nanos()
    }
}

/// A wrapper around [`AtomicI64`] to implement [`AtomicInstant`].
#[derive(Debug)]
pub(crate) struct AtomicStackTime(AtomicI64);

impl AtomicInstant<StackTime> for AtomicStackTime {
    fn new(instant: StackTime) -> AtomicStackTime {
        AtomicStackTime(AtomicI64::new(instant.0.into_nanos()))
    }

    fn load(&self, ordering: Ordering) -> StackTime {
        StackTime(fasync::MonotonicInstant::from_nanos(self.0.load(ordering)))
    }

    fn store(&self, instant: StackTime, ordering: Ordering) {
        self.0.store(instant.0.into_nanos(), ordering);
    }

    fn store_max(&self, instant: StackTime, ordering: Ordering) {
        let _prev = self.0.fetch_max(instant.0.into_nanos(), ordering);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_stack_time() {
        let t1 = StackTime(fasync::MonotonicInstant::from_nanos(1));
        let t2 = StackTime(fasync::MonotonicInstant::from_nanos(1));
        let t3 = StackTime(fasync::MonotonicInstant::from_nanos(1));
        let time = AtomicStackTime::new(t1);
        assert_eq!(time.load(Ordering::Relaxed), t1);
        time.store(t2, Ordering::Relaxed);
        assert_eq!(time.load(Ordering::Relaxed), t2);

        // Verify `store_max`.
        time.store_max(t1, Ordering::Relaxed);
        assert_eq!(time.load(Ordering::Relaxed), t2);
        time.store_max(t3, Ordering::Relaxed);
        assert_eq!(time.load(Ordering::Relaxed), t3);
    }
}
