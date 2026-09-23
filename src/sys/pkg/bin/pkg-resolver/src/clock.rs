// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(not(test))]
pub(crate) fn now() -> zx::MonotonicInstant {
    zx::MonotonicInstant::get()
}

#[cfg(test)]
pub(crate) use mock::now;

#[cfg(test)]
pub(crate) mod mock {
    use std::cell::Cell;

    thread_local!(
        static MOCK_TIME: Cell<zx::MonotonicInstant> = Cell::new(zx::MonotonicInstant::get())
    );

    pub fn now() -> zx::MonotonicInstant {
        MOCK_TIME.with(|time| time.get())
    }

    pub fn set(new_time: zx::MonotonicInstant) {
        MOCK_TIME.with(|time| time.set(new_time));
    }
}
