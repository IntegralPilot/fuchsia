// Copyright 2024 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

use futures::{FutureExt as _, future::BoxFuture};
use omaha_client::time::{PartialComplexTime, Timer};
use std::time::{Duration, SystemTime};

pub struct MinimalTimer;

impl MinimalTimer {
    fn duration_until_system_time(system: SystemTime) -> Duration {
        system.duration_since(SystemTime::now()).ok().unwrap_or(Duration::from_secs(0))
    }

    fn duration_until_instant(instant: std::time::Instant) -> Duration {
        instant.checked_duration_since(std::time::Instant::now()).unwrap_or(Duration::from_secs(0))
    }
}

impl Timer for MinimalTimer {
    /// Wait until at least one of the given time bounds has been reached.
    fn wait_until(&mut self, time: impl Into<PartialComplexTime>) -> BoxFuture<'static, ()> {
        let t: PartialComplexTime = time.into();
        let duration = match t {
            PartialComplexTime::Wall(w) => Self::duration_until_system_time(w),
            PartialComplexTime::Monotonic(m) => Self::duration_until_instant(m),
            PartialComplexTime::Complex(c) => core::cmp::min(
                Self::duration_until_system_time(c.wall),
                Self::duration_until_instant(c.mono),
            ),
        };

        tokio::time::sleep(duration).boxed()
    }

    /// Wait for the given duration (from now).
    fn wait_for(&mut self, duration: Duration) -> BoxFuture<'static, ()> {
        tokio::time::sleep(duration).boxed()
    }
}
