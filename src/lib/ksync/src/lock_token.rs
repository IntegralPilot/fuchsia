// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;

/// A token proving that a lock of lock class `Class` is currently held by the current thread.
///
/// This token acts as a static proof to permit type-safe borrow access to cell values guarded by
/// this lock class (via `KCell`).
pub struct LockToken<'a, Class> {
    _marker: PhantomData<&'a Class>,
    _phantom: PhantomData<*const ()>,
}

impl<'a, Class> LockToken<'a, Class> {
    /// Creates a new proof token for the lock class `Class`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the mutual exclusion lock associated with this lock class is
    /// currently held by the current thread and that the lifetime `'a` is bounded to the duration
    /// of the lock being held. Creating a token when the lock is not held, or letting the token
    /// outlive the lock hold duration, can lead to concurrent mutation or data races, which is
    /// undefined behavior.
    #[inline]
    pub unsafe fn new() -> Self {
        Self { _marker: PhantomData, _phantom: PhantomData }
    }
}
