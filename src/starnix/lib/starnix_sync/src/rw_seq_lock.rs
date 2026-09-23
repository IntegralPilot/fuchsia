// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{
    LockDepGuard, LockDepMutex, LockLevel, ThreadAffinity, ThreadAffinityGuard, assert_lock_level,
};
use fuchsia_rcu::RcuDroppable;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Number of times a reader retries the lock-free path before falling back to taking the lock.
const MAX_SPIN_COUNT: usize = 10;

/// A sequence lock that combines a standard lock (like a Mutex) with a sequence
/// counter. This allows lock-free concurrent reads by spinning if a write is in
/// progress, while still enforcing mutually exclusive writes.
///
/// This lock is used to synchronize threads within the same address space.
/// For synchronizing data across address spaces (e.g. sharing data with
/// userspace via a VMO), see `//src/starnix/lib/seq_lock/`.
#[derive(RcuDroppable)]
pub struct RwSeqLock<L> {
    /// The sequence number. An even value indicates the lock is not currently held
    /// for writing, while an odd value indicates a write is in progress.
    seq: AtomicUsize,
    /// The underlying lock used to serialize writers.
    lock: L,
    /// Tracks if the current thread is currently holding this lock, preventing read_seq
    /// from being called while holding the lock (which would lead to a livelock).
    affinity: ThreadAffinity,
}

/// A guard that manages the sequence counter and wraps the underlying lock guard.
///
/// When this guard is dropped, the sequence counter is incremented, signaling to
/// readers that the write operation has finished.
pub struct RwSeqLockGuard<'a, G> {
    seq: &'a AtomicUsize,
    _affinity: ThreadAffinityGuard<'a>,
    guard: G,
}

impl<L> RwSeqLock<L> {
    /// Creates a new `RwSeqLock` wrapping the provided `lock`.
    pub const fn new(lock: L) -> Self {
        Self { seq: AtomicUsize::new(0), lock, affinity: ThreadAffinity::new() }
    }
}

impl<T, L: LockLevel> RwSeqLock<LockDepMutex<T, L>> {
    /// Executes the given closure `f` and returns its result, guaranteeing that
    /// no writer was holding the lock while the closure was running.
    ///
    /// If a write is in progress, this method spins until the write finishes. If a write begins
    /// while the closure is executing, the closure is retried. After `MAX_SPIN_COUNT` failed
    /// attempts, the underlying lock is taken so that an active writer cannot starve readers.
    ///
    /// Because of that fallback, this method may acquire a lock of level `L`: the caller must be
    /// allowed to acquire `L`, `f` must not acquire a lock at a level lower than or equal to `L`,
    /// and `f` must not call `read_seq` on the same lock. All of this is checked on every call,
    /// not only when the fallback triggers.
    pub fn read_seq<R, F: Fn() -> R>(&self, f: F) -> R {
        self.affinity.assert_not_attached();

        // Check the lock ordering on every call, so that a violation doesn't depend on the timing
        // of the writers.
        let lock_level_guard = assert_lock_level::<L>();

        for _ in 0..MAX_SPIN_COUNT {
            let seq1 = self.seq.load(Ordering::Acquire);
            if seq1 % 2 != 0 {
                // A writer is currently holding the lock.
                std::hint::spin_loop();
                continue;
            }

            let result = f();

            // A read memory barrier is required here to prevent the CPU from reordering
            // the reads inside `f()` to happen AFTER `seq2` is loaded.
            // `seq2.load(Ordering::Acquire)` only prevents subsequent accesses from moving
            // before the load, but does not prevent preceding accesses from moving after it.
            std::sync::atomic::fence(Ordering::Acquire);

            let seq2 = self.seq.load(Ordering::Acquire);
            if seq1 == seq2 {
                // The sequence number hasn't changed, meaning no writer interfered.
                return result;
            }
        }

        // Writers are too active: take the underlying lock, which excludes them for the duration
        // of the closure. Take the inner lock directly, as `lock()` would bump the sequence and
        // force every other reader down this same path. Release the lock level token first: the
        // acquisition does the same ordering check, and lockdep would otherwise report a
        // self-deadlock on level `L`.
        drop(lock_level_guard);
        let _guard = self.lock.lock();
        f()
    }

    /// Acquires the underlying lock for writing.
    ///
    /// This increments the sequence counter (making it odd) to indicate to readers
    /// that a write is in progress. When the returned guard is dropped, the sequence
    /// counter is incremented again (making it even).
    pub fn lock(&self) -> RwSeqLockGuard<'_, LockDepGuard<'_, T>> {
        let guard = self.lock.lock();
        // Increment the sequence to an odd number, notifying readers that writing has
        // started.
        let prev = self.seq.fetch_add(1, Ordering::Release);
        debug_assert!(prev % 2 == 0, "RwSeqLock sequence should be even before locking");
        RwSeqLockGuard { seq: &self.seq, _affinity: self.affinity.attach(), guard }
    }
}

impl<'a, G> Drop for RwSeqLockGuard<'a, G> {
    fn drop(&mut self) {
        // Increment the sequence to an even number, notifying readers that writing is
        // finished.
        let prev = self.seq.fetch_add(1, Ordering::Release);
        debug_assert!(prev % 2 != 0, "RwSeqLock sequence should be odd before unlocking");
    }
}

impl<'a, G: Deref> Deref for RwSeqLockGuard<'a, G> {
    type Target = G::Target;
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl<'a, G: DerefMut> DerefMut for RwSeqLockGuard<'a, G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

impl<L: Default> Default for RwSeqLock<L> {
    fn default() -> Self {
        Self::new(L::default())
    }
}

impl<L: fmt::Debug> fmt::Debug for RwSeqLock<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RwSeqLock").field("lock", &self.lock).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock_ordering;
    use std::sync::atomic::{AtomicU32, Ordering};

    lock_ordering! {
        Unlocked => TestLevel,
    }

    #[test]
    fn test_rw_seq_lock() {
        let lock: RwSeqLock<LockDepMutex<u32, TestLevel>> = RwSeqLock::new(0.into());
        let data = AtomicU32::new(0);

        let read_val = lock.read_seq(|| data.load(Ordering::Relaxed));
        assert_eq!(read_val, 0);

        {
            let mut guard = lock.lock();
            *guard = 1;
            data.store(1, Ordering::Relaxed);
        }

        let read_val2 = lock.read_seq(|| data.load(Ordering::Relaxed));
        assert_eq!(read_val2, 1);
    }

    #[test]
    fn test_rw_seq_lock_read_falls_back_to_locking() {
        use std::sync::{Arc, Barrier};
        use std::thread;
        use std::time::Duration;

        struct TestData {
            lock: RwSeqLock<LockDepMutex<(), TestLevel>>,
            val: AtomicU32,
            barrier: Barrier,
        }

        let data = Arc::new(TestData {
            lock: RwSeqLock::new(Default::default()),
            val: AtomicU32::new(0),
            barrier: Barrier::new(2),
        });

        // Hold the write lock for as long as the reader runs, so that every lock-free attempt
        // fails and the reader is forced down the fallback path.
        let guard = data.lock.lock();

        let reader = thread::spawn({
            let data = data.clone();
            move || {
                data.barrier.wait();
                data.lock.read_seq(|| data.val.load(Ordering::Relaxed))
            }
        });

        data.barrier.wait();
        // Leave the reader enough time to exhaust its attempts and block on the lock. The test
        // stays correct otherwise, it just stops covering the fallback path.
        thread::sleep(Duration::from_millis(50));

        data.val.store(42, Ordering::Relaxed);
        drop(guard);

        assert_eq!(reader.join().unwrap(), 42);
    }

    #[test]
    fn test_rw_seq_lock_concurrent() {
        use std::sync::Arc;
        use std::thread;

        struct TestData {
            lock: RwSeqLock<LockDepMutex<(), TestLevel>>,
            val1: AtomicU32,
            val2: AtomicU32,
        }

        let data = Arc::new(TestData {
            lock: RwSeqLock::new(Default::default()),
            val1: AtomicU32::new(0),
            val2: AtomicU32::new(0),
        });

        let mut handles = vec![];

        // Spawn writers
        for i in 0..4 {
            let data = data.clone();
            handles.push(thread::spawn(move || {
                for j in 0..1000 {
                    let val = i * 1000 + j;
                    let _guard = data.lock.lock();
                    data.val1.store(val, Ordering::Relaxed);
                    thread::yield_now();
                    data.val2.store(val, Ordering::Relaxed);
                }
            }));
        }

        // Spawn readers
        for _ in 0..4 {
            let data = data.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    let (v1, v2) = data.lock.read_seq(|| {
                        let v1 = data.val1.load(Ordering::Relaxed);
                        thread::yield_now();
                        let v2 = data.val2.load(Ordering::Relaxed);
                        (v1, v2)
                    });
                    assert_eq!(v1, v2);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }
    }
}
