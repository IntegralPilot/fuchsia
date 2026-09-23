// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::cell::Cell;

use crate::mutex::raw::RawMutex;

/// A zero-cost mutex implementation for single-threaded execution contexts.
///
/// Panics on recursive lock attempts to prevent reentrancy deadlocks.
///
/// Because it uses `Cell` internally, it is `Send` but `!Sync`.
///
/// This allows it to be safely moved to another thread (as ownership is transferred),
/// but prevents multiple threads from holding concurrent references to it.
///
/// # Examples
///
/// A `SingleThreadMutex` can be safely sent to another thread:
///
/// ```
/// use sapphire_sync::mutex::raw::SingleThreadMutex;
/// use std::thread;
///
/// let mtx = SingleThreadMutex::new();
/// thread::spawn(move || {
///     let _ = mtx; // Compiles successfully!
/// });
/// ```
///
/// A `SingleThreadMutex` cannot be shared across threads (compile_fail):
///
/// ```compile_fail
/// use sapphire_sync::mutex::raw::SingleThreadMutex;
///
/// fn assert_sync<T: Sync>() {}
/// assert_sync::<SingleThreadMutex>(); // Correctly fails to compile because SingleThreadMutex is !Sync
/// ```
pub struct Mutex {
    borrowed: Cell<bool>,
}

impl Default for Mutex {
    fn default() -> Self {
        Self::new()
    }
}
impl Mutex {
    /// Creates a new unlocked `Mutex`.
    pub const fn new() -> Self {
        Self { borrowed: Cell::new(false) }
    }
}
// SAFETY: `SingleThreadMutex` uses `Cell<bool>` to ensure mutual exclusion on a single thread.
// It is not `Sync`, preventing multithreaded data races.
unsafe impl RawMutex for Mutex {
    fn lock(&self) {
        if self.borrowed.get() {
            panic!("Attempting to lock single-thread mutex twice is a deadlock");
        }
        self.borrowed.set(true);
    }
    unsafe fn unlock(&self) {
        assert!(self.borrowed.get(), "Attempt to unlock a mutex that was never locked");
        self.borrowed.set(false);
    }
}
