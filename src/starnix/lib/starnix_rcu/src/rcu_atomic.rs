// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_sync::{LockDepGuard, LockDepMutex, LockLevel};
use starnix_types::atomic::{AsAtomic, AtomicOperations};
use std::ops::{Deref, DerefMut};
use std::sync::atomic::Ordering;

pub struct RcuAtomic<T: AsAtomic, L: LockLevel> {
    mutex: LockDepMutex<T, L>,
    atomic: T::Atomic,
}

impl<T: AsAtomic + std::fmt::Debug, L: LockLevel> std::fmt::Debug for RcuAtomic<T, L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RcuAtomic").field("value", &self.read()).finish()
    }
}

impl<T: AsAtomic, L: LockLevel> RcuAtomic<T, L> {
    pub fn new(value: T) -> Self {
        Self { mutex: value.into(), atomic: T::Atomic::new(value) }
    }

    /// Read the value from the atomic without taking the lock.
    pub fn read(&self) -> T {
        self.atomic.load(Ordering::Relaxed)
    }

    /// Basic write operation: takes the mutex and updates both the mutex value and the atomic.
    pub fn write(&self, value: T) {
        let mut guard = self.mutex.lock();
        *guard = value;
        self.atomic.store(value, Ordering::Relaxed);
    }

    /// Takes the mutex and returns a guard that can be used to read and modify the value.
    /// Updates are only committed to the atomic when `update` is called on the guard.
    pub fn copy(&self) -> RcuAtomicGuard<'_, T, L> {
        let guard = self.mutex.lock();
        RcuAtomicGuard { parent: self, guard }
    }
}

pub struct RcuAtomicGuard<'a, T: AsAtomic, L: LockLevel> {
    parent: &'a RcuAtomic<T, L>,
    guard: LockDepGuard<'a, T>,
}

impl<'a, T: AsAtomic, L: LockLevel> Deref for RcuAtomicGuard<'a, T, L> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &*self.guard
    }
}

impl<'a, T: AsAtomic, L: LockLevel> DerefMut for RcuAtomicGuard<'a, T, L> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.guard
    }
}

impl<'a, T: AsAtomic, L: LockLevel> RcuAtomicGuard<'a, T, L> {
    /// Consumes the guard, updates the atomic in `RcuAtomic` with the current value,
    /// and drops the mutex guard.
    pub fn update(self) {
        let value = *self.guard;
        self.parent.atomic.store(value, Ordering::Relaxed);
        // guard dropped here
    }
}
