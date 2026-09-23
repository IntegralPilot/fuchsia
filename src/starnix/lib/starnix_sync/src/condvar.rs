// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::MutexGuard;

/// A Condition Variable that is compatible with both standard Mutex and LockDepMutex
/// in Starnix.
pub struct CondVar {
    inner: fuchsia_sync::Condvar,
}

/// A token that proves the caller is allowed to access the inner guard.
/// Its field is private, so it can only be constructed within this crate.
pub struct WaitToken(());

pub trait WaitableMutexGuard<'a, T> {
    fn inner_guard(&mut self, token: WaitToken) -> &mut MutexGuard<'a, T>;
}

impl<'a, T> WaitableMutexGuard<'a, T> for MutexGuard<'a, T> {
    fn inner_guard(&mut self, _token: WaitToken) -> &mut MutexGuard<'a, T> {
        self
    }
}

impl CondVar {
    #[inline]
    pub const fn new() -> Self {
        Self { inner: fuchsia_sync::Condvar::new() }
    }

    /// Blocks the current thread until this condition variable receives a notification.
    pub fn wait<'a, T: 'a, G: WaitableMutexGuard<'a, T>>(&self, guard: &mut G) {
        self.inner.wait(guard.inner_guard(WaitToken(())));
    }

    /// Wakes up one blocked thread on this condvar.
    pub fn notify_one(&self) {
        self.inner.notify_one();
    }

    /// Wakes up all blocked threads on this condvar.
    pub fn notify_all(&self) {
        self.inner.notify_all();
    }
}

impl Default for CondVar {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CondVar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CondVar").finish()
    }
}
