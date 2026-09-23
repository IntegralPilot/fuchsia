// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::Mutex;
use std::sync::Arc;

/// TimeSource provides the current time in nanoseconds since the Unix epoch.
/// A `&'a dyn TimeSource` can be injected into a data structure.
/// TimeSource is implemented by UtcInstant for wall-clock system time, and
/// FakeTime for a clock that is explicitly set by testing code.
pub trait TimeSource: std::fmt::Debug {
    fn now(&self) -> i64;
}

/// FakeTime instances return the last value that was configured by testing code via `set_ticks()`
///  or `add_ticks()`. Upon initialization, they return 0.
#[derive(Clone, Debug)]
pub struct FakeTime {
    time: Arc<Mutex<i64>>,
}

impl TimeSource for FakeTime {
    fn now(&self) -> i64 {
        *self.time.lock()
    }
}

impl FakeTime {
    pub fn new() -> FakeTime {
        FakeTime { time: Arc::new(Mutex::new(0)) }
    }

    pub fn set_ticks(&self, now: i64) {
        *self.time.lock() = now;
    }

    pub fn add_ticks(&self, ticks: i64) {
        *self.time.lock() += ticks;
    }
}

/// IncrementingFakeTime automatically increments itself by `increment_by`
/// before each call to `self.now()`.
#[derive(Debug)]
pub struct IncrementingFakeTime {
    time: FakeTime,
    increment_by: std::time::Duration,
}

impl TimeSource for IncrementingFakeTime {
    fn now(&self) -> i64 {
        let now = self.time.now();
        self.time.add_ticks(self.increment_by.as_nanos() as i64);
        now
    }
}

impl IncrementingFakeTime {
    pub fn new(start_time: i64, increment_by: std::time::Duration) -> Self {
        let time = FakeTime::new();
        time.set_ticks(start_time);
        Self { time, increment_by }
    }
}

/// UtcInstant instances return the Rust system clock value each time now() is called.
#[derive(Debug)]
pub struct UtcInstant {}

impl UtcInstant {
    pub fn new() -> UtcInstant {
        UtcInstant {}
    }
}

impl TimeSource for UtcInstant {
    fn now(&self) -> i64 {
        if cfg!(target_arch = "wasm32") {
            // TODO(https://fxbug.dev/42143658): Remove this when WASM avoids calling this method.
            0i64
        } else {
            let now_utc = chrono::prelude::Utc::now(); // Consider using SystemTime::now()?
            now_utc.timestamp() * 1_000_000_000 + now_utc.timestamp_subsec_nanos() as i64
        }
    }
}

/// MonotonicInstant instances provide a monotonic clock.
/// On Fuchsia, MonotonicInstant uses zx::MonotonicInstant::get().
#[derive(Debug)]
pub struct MonotonicInstant {
    #[cfg(not(target_os = "fuchsia"))]
    starting_time: std::time::Instant,
}

impl MonotonicInstant {
    pub fn new() -> MonotonicInstant {
        #[cfg(target_os = "fuchsia")]
        let time = MonotonicInstant {};
        #[cfg(not(target_os = "fuchsia"))]
        let time = MonotonicInstant { starting_time: std::time::Instant::now() };

        time
    }
}

impl TimeSource for MonotonicInstant {
    fn now(&self) -> i64 {
        #[cfg(target_os = "fuchsia")]
        let now = zx::MonotonicInstant::get().into_nanos();
        #[cfg(not(target_os = "fuchsia"))]
        let now = (std::time::Instant::now() - self.starting_time).as_nanos() as i64;

        now
    }
}

/// BootInstant instances provide a monotonic clock.
/// On Fuchsia, BootInstant uses zx::BootInstant::get().
#[derive(Debug)]
pub struct BootInstant {
    #[cfg(not(target_os = "fuchsia"))]
    starting_time: std::time::Instant,
}

impl BootInstant {
    pub fn new() -> BootInstant {
        #[cfg(target_os = "fuchsia")]
        let time = BootInstant {};
        #[cfg(not(target_os = "fuchsia"))]
        let time = BootInstant { starting_time: std::time::Instant::now() };

        time
    }
}

impl TimeSource for BootInstant {
    fn now(&self) -> i64 {
        #[cfg(target_os = "fuchsia")]
        let now = zx::BootInstant::get().into_nanos();
        #[cfg(not(target_os = "fuchsia"))]
        let now = (std::time::Instant::now() - self.starting_time).as_nanos() as i64;

        now
    }
}

#[cfg(test)]
mod test {

    use super::*;

    struct TimeHolder<'a> {
        time_source: &'a dyn TimeSource,
    }

    impl<'a> TimeHolder<'a> {
        fn new(time_source: &'a dyn TimeSource) -> TimeHolder<'_> {
            TimeHolder { time_source }
        }

        fn now(&self) -> i64 {
            self.time_source.now()
        }
    }

    #[test]
    fn test_system_time() {
        let time_source = UtcInstant::new();
        let time_holder = TimeHolder::new(&time_source);
        let first_time = time_holder.now();
        // Make sure the system time is ticking. If not, this will hang until the test times out.
        while time_holder.now() == first_time {}
    }

    #[test]
    fn test_monotonic_time() {
        let time_source = MonotonicInstant::new();
        let time_holder = TimeHolder::new(&time_source);
        let first_time = time_holder.now();
        // Make sure the monotonic time is ticking. If not, this will hang until the test times out.
        while time_holder.now() == first_time {}
    }

    #[test]
    fn test_boot_time() {
        let time_source = BootInstant::new();
        let time_holder = TimeHolder::new(&time_source);
        let first_time = time_holder.now();
        // Make sure the monotonic time is ticking. If not, this will hang until the test times out.
        while time_holder.now() == first_time {}
    }

    #[test]
    fn test_fake_time() {
        let time_source = FakeTime::new();
        let time_holder = TimeHolder::new(&time_source);

        // Fake time is 0 on initialization.
        let time_0 = time_holder.now();
        time_source.set_ticks(1000);
        let time_1000 = time_holder.now();
        // Fake time does not auto-increment.
        let time_1000_2 = time_holder.now();
        // Fake time can go backward.
        time_source.set_ticks(500);
        let time_500 = time_holder.now();
        // add_ticks() works.
        time_source.add_ticks(123);
        let time_623 = time_holder.now();
        // add_ticks() can take a negative value
        time_source.add_ticks(-23);
        let time_600 = time_holder.now();

        assert_eq!(time_0, 0);
        assert_eq!(time_1000, 1000);
        assert_eq!(time_1000_2, 1000);
        assert_eq!(time_500, 500);
        assert_eq!(time_623, 623);
        assert_eq!(time_600, 600);
    }

    #[test]
    fn test_incrementing_fake_time() {
        let duration = std::time::Duration::from_nanos(1000);
        let timer = IncrementingFakeTime::new(0, duration);

        assert_eq!(0, timer.now());
        assert_eq!((1 * duration).as_nanos() as i64, timer.now());
        assert_eq!((2 * duration).as_nanos() as i64, timer.now());
    }
}
