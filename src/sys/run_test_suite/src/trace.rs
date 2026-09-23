// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(target_os = "fuchsia")]
macro_rules! duration {
    ($name:expr) => {
        ::fuchsia_trace::duration!("run_test_suite", $name);
    };
}

// On host we'll measure durations manually, then emit a trace level log.
#[cfg(not(target_os = "fuchsia"))]
macro_rules! duration {
    ($name:literal) => {
        let _scope = crate::trace::DurationScope::new($name);
    };
}

#[cfg(not(target_os = "fuchsia"))]
pub(crate) struct DurationScope {
    start: std::time::Instant,
    name: &'static str,
}

#[cfg(not(target_os = "fuchsia"))]
impl DurationScope {
    pub(crate) fn new(name: &'static str) -> Self {
        Self { name, start: std::time::Instant::now() }
    }
}

#[cfg(not(target_os = "fuchsia"))]
impl std::ops::Drop for DurationScope {
    fn drop(&mut self) {
        log::trace!(
            name = self.name,
            duration = self.start.elapsed().as_nanos() as u64;
            "DURATION_NANOS"
        );
    }
}

pub(crate) use duration;
