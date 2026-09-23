// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! An RCU (Read-Copy-Update) data structure.

use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use arc_swap::ArcSwap;
use netstack3_core::sync::{LockGuard, Mutex};

/// An RCU (Read-Copy-Update) data structure that uses a `Mutex` to synchronize
/// writers.
#[derive(Default)]
pub struct SynchronizedWriterRcu<T> {
    lock: Mutex<()>,
    data: ArcSwap<T>,
}

impl<T> SynchronizedWriterRcu<T> {
    /// Creates a new `SingleWriterRcu` with `value`.
    pub fn new(value: T) -> Self {
        Self { lock: Mutex::new(()), data: ArcSwap::new(Arc::new(value)) }
    }

    /// Acquires a read guard on the RCU.
    pub fn read(&self) -> ReadGuard<'_, T> {
        ReadGuard { guard: self.data.load(), _marker: PhantomData }
    }

    /// Acquires a write guard on the RCU by cloning `T`.
    ///
    /// See [`WriteGuard::write_with``].
    pub fn write(&self) -> WriteGuard<'_, T>
    where
        T: Clone,
    {
        self.write_with(T::clone)
    }

    /// Acquires a write guard on the RCU.
    ///
    /// [`WriteGuard`] provides mutable access to a new copy of the data kept by
    /// `SingleWriterRcu` via `f`.
    ///
    /// Dropping the returned [`WriteGuard`] commits any changes back to the
    /// RCU. Changes may be discarded with [`WriteGuard::discard`].
    pub fn write_with<F: FnOnce(&T) -> T>(&self, f: F) -> WriteGuard<'_, T> {
        let Self { lock, data } = self;
        // Lock before reading the data.
        let lock_guard = lock.lock();
        let copy = f(&*data.load());
        WriteGuard(ManuallyDrop::new(WriteGuardInner { copy, lock_guard, data: &self.data }))
    }
}

/// A read guard on [`SingleWriterRcu`].
///
/// Implements [`Deref`] to get to the contained data.
pub struct ReadGuard<'a, T> {
    guard: arc_swap::Guard<Arc<T>>,
    _marker: PhantomData<&'a ()>,
}

impl<'a, T> Deref for ReadGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &*self.guard
    }
}

/// A write guard on [`SingleWriterRcu`].
///
/// Implements [`Deref`]  and [`DerefMut`] to get to the contained data.
///
/// Changes to the contained data are applied back to the RCU on drop.
pub struct WriteGuard<'a, T>(ManuallyDrop<WriteGuardInner<'a, T>>);

struct WriteGuardInner<'a, T> {
    copy: T,
    data: &'a ArcSwap<T>,
    lock_guard: LockGuard<'a, ()>,
}

impl<'a, T> Deref for WriteGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        let Self(inner) = self;
        &inner.copy
    }
}

impl<'a, T> DerefMut for WriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        let Self(inner) = self;
        &mut inner.copy
    }
}

impl<'a, T> WriteGuard<'a, T> {
    /// Discards this attempt at writing to the RCU. No changes will be
    /// committed.
    pub fn discard(mut self) {
        // Drop the inner slot without dropping self.
        let Self(inner) = &mut self;
        // SAFETY: inner is not used again. We prevent the inner drop
        // implementation from running by forgetting self.
        unsafe {
            ManuallyDrop::drop(inner);
        }
        std::mem::forget(self);
    }
}

impl<'a, T> Drop for WriteGuard<'a, T> {
    fn drop(&mut self) {
        let Self(inner) = self;
        // SAFETY: inner is not used again, self is being dropped.
        let WriteGuardInner { copy, data, lock_guard } = unsafe { ManuallyDrop::take(inner) };
        data.store(Arc::new(copy));
        // Only drop the lock once we're done.
        std::mem::drop(lock_guard);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn race_writers() {
        const ROUNDS: usize = 100;
        let rcu = Arc::new(SynchronizedWriterRcu::new(0usize));
        let rcu_clone = rcu.clone();
        let writer = move || {
            let mut last = None;
            for _ in 0..ROUNDS {
                let mut data = rcu_clone.write();
                assert!(last.is_none_or(|l| *data > l));
                last = Some(*data);
                *data += 1;
            }
        };
        let w1 = std::thread::spawn(writer.clone());
        let w2 = std::thread::spawn(writer);
        let rcu_clone = rcu.clone();
        let reader = std::thread::spawn(move || {
            let mut last = None;
            for _ in 0..(ROUNDS * 2) {
                let data = rcu_clone.read();
                assert!(last.is_none_or(|l| *data >= l));
                last = Some(*data);
            }
        });

        w1.join().expect("join w1");
        w2.join().expect("join w2");
        reader.join().expect("join reader");
        assert_eq!(*rcu.read(), ROUNDS * 2);
    }

    #[test]
    fn read_guard_post_write() {
        let rcu = SynchronizedWriterRcu::new(0usize);
        let read1 = rcu.read();
        assert_eq!(*read1, 0);
        let mut write = rcu.write();
        *write = 1;
        // Drop to commit.
        std::mem::drop(write);
        let read2 = rcu.read();
        assert_eq!(*read1, 0);
        assert_eq!(*read2, 1);
    }

    #[test]
    fn write_guard_discard() {
        let rcu = SynchronizedWriterRcu::new(0usize);
        let read1 = rcu.read();
        assert_eq!(*read1, 0);
        let mut write = rcu.write();
        *write = 1;
        write.discard();
        let read2 = rcu.read();
        assert_eq!(*read1, 0);
        assert_eq!(*read2, 0);
    }
}
