// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use parking_lot::RawFairMutex;
use parking_lot::lock_api::RawMutex as _;

use crate::mutex::raw::RawMutex;

/// Multi-threaded mutex implementation based on a [`parking_lot::RawFairMutex`]
pub struct Mutex {
    lock: RawFairMutex,
}

impl Default for Mutex {
    fn default() -> Self {
        Self::new()
    }
}
impl Mutex {
    /// Creates a new unlocked `Mutex`.
    pub const fn new() -> Self {
        Self { lock: RawFairMutex::INIT }
    }
}

// SAFETY: parking_lot::RawMutex has the same internal guarantees
unsafe impl RawMutex for Mutex {
    fn lock(&self) {
        self.lock.lock();
    }

    unsafe fn unlock(&self) {
        // SAFETY: RawMutex precondition requires that `self` is locked.
        unsafe { self.lock.unlock() }
    }
}
