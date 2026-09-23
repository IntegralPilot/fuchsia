// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(target_os = "fuchsia")]
mod fuchsia {
    pub use zx::{
        BootInstant, BootTicks, Instant, MonotonicInstant, MonotonicTicks, NsUnit, Ticks,
        TicksUnit, Timeline,
    };
}
#[cfg(target_os = "fuchsia")]
pub use fuchsia::*;

#[cfg(not(target_os = "fuchsia"))]
mod host {
    // This module provides a small subset of our Zircon Time bindings for the purposes of
    // encoding/decoding times in the host without losing the type safety we get on the target.
    use std::marker::PhantomData;
    use zx_types::{zx_ticks_t, zx_time_t};

    /// A marker trait for times to prevent accidental comparison between different timelines.
    pub trait Timeline {}

    /// A marker type representing nanoseconds.
    #[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub struct NsUnit;

    /// A marker type representing ticks.
    #[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub struct TicksUnit;

    /// A marker type for the system's monotonic timeline which pauses during suspend.
    #[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub struct MonotonicTimeline;
    impl Timeline for MonotonicTimeline {}

    /// A marker type for the system's boot timeline which continues running during suspend.
    #[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub struct BootTimeline;
    impl Timeline for BootTimeline {}

    /// Time generic over the Timeline.
    #[repr(transparent)]
    #[derive(Debug, Copy, Clone, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub struct Instant<T, U = NsUnit>(i64, PhantomData<(T, U)>);

    /// Monotonic timestamp (ns).
    pub type MonotonicInstant = Instant<MonotonicTimeline, NsUnit>;
    /// Boot timestamp (ns).
    pub type BootInstant = Instant<BootTimeline, NsUnit>;
    /// A timestamp from system ticks on the monotonic timeline. Does not advance while the system is
    /// suspended.
    pub type MonotonicTicks = Instant<MonotonicTimeline, TicksUnit>;
    /// A timestamp from system ticks on the boot timeline. Advances while the system is suspended.
    pub type BootTicks = Instant<BootTimeline, TicksUnit>;
    /// A timestamp from system ticks.
    pub type Ticks<T> = Instant<T, TicksUnit>;

    impl<T, U> Instant<T, U> {
        /// Zero timestamp.
        pub const ZERO: Instant<T, U> = Instant(0, PhantomData);
    }

    impl<T: Timeline> Instant<T> {
        /// Create a timestamp from nanoseconds.
        pub const fn from_nanos(nanos: zx_time_t) -> Self {
            Self(nanos, PhantomData)
        }

        /// Return the number of nanoseconds associated with this timestamp.
        pub fn into_nanos(self) -> zx_time_t {
            self.0
        }
    }

    impl<T: Timeline> Instant<T, TicksUnit> {
        /// Create an instant from ticks.
        pub const fn from_raw(nanos: zx_ticks_t) -> Self {
            Self(nanos, PhantomData)
        }

        /// Return the number of ticks associated with this instant.
        pub fn into_raw(self) -> zx_ticks_t {
            self.0
        }
    }
}

#[cfg(not(target_os = "fuchsia"))]
pub use host::*;
