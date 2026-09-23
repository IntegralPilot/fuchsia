// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// Uptime-related methods from the [OpenThread "Instance" Module][1].
///
/// [1]: https://openthread.io/reference/group/api-instance
pub trait Uptime {
    /// Functional equivalent of [`otsys::otInstanceGetUptime`](crate::otsys::otInstanceGetUptime).
    fn get_uptime(&self) -> zx::MonotonicDuration;
}

impl<T: Uptime + Boxable> Uptime for ot::Box<T> {
    fn get_uptime(&self) -> zx::MonotonicDuration {
        self.as_ref().get_uptime()
    }
}

impl Uptime for Instance {
    fn get_uptime(&self) -> zx::MonotonicDuration {
        unsafe {
            zx::MonotonicDuration::from_millis(
                otInstanceGetUptime(self.as_ot_ptr()).try_into().unwrap(),
            )
        }
    }
}
