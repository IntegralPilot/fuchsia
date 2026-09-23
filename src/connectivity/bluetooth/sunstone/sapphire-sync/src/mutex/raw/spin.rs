// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::mutex::raw::RawMutex;

/// A lightweight atomic spinlock implementation for multithreaded execution contexts.
pub struct Mutex {
    locked: AtomicBool,
}

impl Mutex {
    /// Creates a new unlocked `SpinMutex`.
    pub const fn new() -> Self {
        Self { locked: AtomicBool::new(false) }
    }
}

impl Default for Mutex {
    fn default() -> Self {
        Self::new()
    }
}
// SAFETY: Uses an atomic compare-and-exchange loop with Acquire/Release ordering,
// ensuring correct memory visibility and strict mutual exclusion across OS threads.
unsafe impl RawMutex for Mutex {
    fn lock(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }
    unsafe fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}
