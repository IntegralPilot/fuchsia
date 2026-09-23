// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Raw mutex implementations defining mutual exclusion semantics.

mod single_thread;
mod spin;

#[cfg(feature = "std")]
mod std;

pub use single_thread::Mutex as SingleThreadMutex;
pub use spin::Mutex as SpinMutex;

#[cfg(feature = "std")]
pub use std::Mutex as StdMutex;

/// A raw mutex trait with locking semantics.
///
/// # Safety
///
/// Implementors must guarantee mutual exclusion and memory visibility appropriate for their execution context.
pub unsafe trait RawMutex: Default {
    /// Acquires the lock, blocking or panicking if already locked.
    fn lock(&self);
    /// Releases the lock.
    ///
    /// # Safety
    /// The caller must guarantee that the mutex is currently locked by the current execution context.
    unsafe fn unlock(&self);
}
