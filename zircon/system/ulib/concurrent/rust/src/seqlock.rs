// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomPinned;
use core::sync::atomic::{AtomicU32, Ordering, fence};

use crate::common::{SYNC_OPT_ACQ_REL_OPS, SYNC_OPT_FENCE, SyncOpt};

#[cfg(feature = "kernel")]
unsafe extern "C" {
    fn cpp_arch_yield();
}

/// Corresponds to `Osal::ArchYield()` in the C++ implementation.
#[inline(always)]
fn arch_yield() {
    #[cfg(feature = "kernel")]
    // SAFETY: `cpp_arch_yield` has no preconditions; it only issues the architecture's
    // yield/pause hint instruction.
    unsafe {
        cpp_arch_yield()
    }
}

pub type SequenceNumber = u32;

const SEQ_NUM_WRITE_IN_FLIGHT: SequenceNumber = 0x1;

const fn write_in_flight(seq_num: SequenceNumber) -> bool {
    (seq_num & SEQ_NUM_WRITE_IN_FLIGHT) != 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadTransactionToken(SequenceNumber);

impl ReadTransactionToken {
    pub const fn new() -> Self {
        Self(1)
    }

    pub const fn seq_num(&self) -> SequenceNumber {
        self.0
    }
}

impl Default for ReadTransactionToken {
    fn default() -> Self {
        Self::new()
    }
}

// Stands in for the C++ __TA_CAPABILITY / __TA_ACQUIRE / __TA_RELEASE
// annotations; dropping the guard releases the lock.
#[must_use = "the write cycle ends as soon as the guard is dropped"]
#[derive(Debug)]
pub struct WriteGuard<'a, const SYNC_OPT: u8 = SYNC_OPT_FENCE> {
    lock: &'a SeqLock<SYNC_OPT>,
}

impl<const SYNC_OPT: u8> Drop for WriteGuard<'_, SYNC_OPT> {
    #[inline]
    fn drop(&mut self) {
        self.lock.release();
    }
}

#[repr(transparent)]
pub struct SeqLock<const SYNC_OPT: u8 = SYNC_OPT_FENCE> {
    seq_num: AtomicU32,
    // No copy, no move
    _pin: PhantomPinned,
}

impl<const SYNC_OPT: u8> SeqLock<SYNC_OPT> {
    // Mirrors the C++ kSyncOpt.
    pub const SYNC_OPT: SyncOpt = SyncOpt::from_u8(SYNC_OPT);

    // Mirrors the C++ kCopyWrapperSyncOpt.
    pub const COPY_WRAPPER_SYNC_OPT: SyncOpt =
        if SYNC_OPT == SYNC_OPT_ACQ_REL_OPS { SyncOpt::AcqRelOps } else { SyncOpt::None };

    const VALID_SYNC_OPT: () = assert!(
        (SYNC_OPT == SYNC_OPT_ACQ_REL_OPS) || (SYNC_OPT == SYNC_OPT_FENCE),
        "The synchronization options chosen for a SeqLock must be either Acquire/Release, or Fence"
    );

    pub const fn new() -> Self {
        let () = Self::VALID_SYNC_OPT;
        Self { seq_num: AtomicU32::new(0), _pin: PhantomPinned }
    }

    // Provide read access to the current seq_num state for testing.
    #[cfg(test)]
    #[inline]
    #[must_use]
    pub fn seq_num(&self) -> SequenceNumber {
        self.seq_num_with_order(Ordering::Relaxed)
    }

    // Provide read access to the current seq_num state with a specified memory order for testing.
    #[cfg(test)]
    #[inline]
    #[must_use]
    pub fn seq_num_with_order(&self, order: Ordering) -> SequenceNumber {
        self.seq_num.load(order)
    }

    // Read Transactions (eg; "locking" for read)

    #[inline]
    #[must_use]
    pub fn begin_read_transaction(&self) -> ReadTransactionToken {
        loop {
            let seq_num = self.seq_num.load(Ordering::Acquire);
            if !write_in_flight(seq_num) {
                return ReadTransactionToken(seq_num);
            }
            arch_yield();
        }
    }

    // The zero-timeout form of the C++ TryBeginReadTransaction.
    #[inline]
    #[must_use]
    pub fn try_begin_read_transaction(&self) -> Option<ReadTransactionToken> {
        let seq_num = self.seq_num.load(Ordering::Acquire);
        if write_in_flight(seq_num) {
            return None;
        }
        Some(ReadTransactionToken(seq_num))
    }

    #[inline]
    #[must_use = "a read transaction which was not validated may have observed a torn payload"]
    pub fn end_read_transaction(&self, token: ReadTransactionToken) -> bool {
        if write_in_flight(token.0) {
            return false;
        }

        // If we are using fence-to-fence synchronization, this is the place we
        // need to put our acquire fence.
        if Self::SYNC_OPT == SyncOpt::Fence {
            fence(Ordering::Acquire);
        }

        self.seq_num.load(Ordering::Relaxed) == token.0
    }

    // The Rust form of the retry loop which C++ readers write out by hand.
    #[inline]
    pub fn read_transaction<T>(&self, mut read_payload: impl FnMut() -> T) -> T {
        loop {
            let token = self.begin_read_transaction();
            let payload = read_payload();
            if self.end_read_transaction(token) {
                return payload;
            }
        }
    }

    // Exclusive locking.

    #[inline]
    pub fn acquire(&self) -> WriteGuard<'_, SYNC_OPT> {
        loop {
            // Wait until we observe an even sequence number.
            let mut expected = self.seq_num.load(Ordering::Relaxed);
            while write_in_flight(expected) {
                arch_yield();
                expected = self.seq_num.load(Ordering::Relaxed);
            }

            // Attempt to increment the even number we observed to be an odd number,
            // with Acquire semantics on the RMW if we succeed.
            if self
                .seq_num
                .compare_exchange(
                    expected,
                    expected.wrapping_add(1),
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                // If we are using fence-to-fence synchronization, this is the place we
                // need to put our release fence.
                if Self::SYNC_OPT == SyncOpt::Fence {
                    fence(Ordering::Release);
                }
                return WriteGuard { lock: self };
            }
        }
    }

    // The zero-timeout form of the C++ TryAcquire.
    #[inline]
    #[must_use]
    pub fn try_acquire(&self) -> Option<WriteGuard<'_, SYNC_OPT>> {
        // Wait until we observe an even sequence number.
        let expected = self.seq_num.load(Ordering::Relaxed);
        if write_in_flight(expected) {
            return None;
        }

        // Attempt to increment the even number we observed to be an odd number,
        // with Acquire semantics on the RMW if we succeed.
        if self
            .seq_num
            .compare_exchange(
                expected,
                expected.wrapping_add(1),
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            // If we are using fence-to-fence synchronization, this is the place we
            // need to put our release fence.
            if Self::SYNC_OPT == SyncOpt::Fence {
                fence(Ordering::Release);
            }
            Some(WriteGuard { lock: self })
        } else {
            None
        }
    }

    #[inline]
    pub(crate) fn release(&self) {
        let before = self.seq_num.fetch_add(1, Ordering::Release);
        debug_assert!(
            write_in_flight(before),
            "SeqLock was not held when the write guard was dropped"
        );
    }
}

impl<const SYNC_OPT: u8> Default for SeqLock<SYNC_OPT> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const SYNC_OPT: u8> core::fmt::Debug for SeqLock<SYNC_OPT> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SeqLock")
            .field("sync_opt", &Self::SYNC_OPT)
            .field("seq_num", &self.seq_num.load(Ordering::Relaxed))
            .finish()
    }
}

// Matches the static_asserts in //zircon/system/ulib/concurrent/tests/seqlock.cc.
zr::static_assert!(core::mem::size_of::<SeqLock>() == 4);
zr::static_assert!(core::mem::align_of::<SeqLock>() == 4);
zr::static_assert!(core::mem::size_of::<SeqLock<SYNC_OPT_ACQ_REL_OPS>>() == 4);
zr::static_assert!(core::mem::align_of::<SeqLock<SYNC_OPT_ACQ_REL_OPS>>() == 4);

#[cfg(test)]
mod tests {
    use super::*;

    type AcqRelSeqLock = SeqLock<SYNC_OPT_ACQ_REL_OPS>;

    #[test]
    fn test_initial_state() {
        let lock: SeqLock = SeqLock::new();
        assert_eq!(lock.seq_num(), 0);
        assert_eq!(SeqLock::<SYNC_OPT_FENCE>::default().seq_num(), 0);
        assert_eq!(AcqRelSeqLock::new().seq_num(), 0);
    }

    #[test]
    fn test_sync_opt() {
        assert_eq!(SeqLock::<SYNC_OPT_FENCE>::SYNC_OPT, SyncOpt::Fence);
        assert_eq!(SeqLock::<SYNC_OPT_FENCE>::COPY_WRAPPER_SYNC_OPT, SyncOpt::None);
        assert_eq!(AcqRelSeqLock::SYNC_OPT, SyncOpt::AcqRelOps);
        assert_eq!(AcqRelSeqLock::COPY_WRAPPER_SYNC_OPT, SyncOpt::AcqRelOps);
    }

    #[test]
    fn test_uncontested_read() {
        let lock: SeqLock = SeqLock::new();

        // With no writer, read transactions should always succeed.
        let token1 = lock.begin_read_transaction();
        assert!(lock.end_read_transaction(token1));

        // A second transaction with no write in-between should also succeed, and the
        // reported sequence number should be unchanged.
        let token2 = lock.begin_read_transaction();
        assert!(lock.end_read_transaction(token2));
        assert_eq!(token1.seq_num(), token2.seq_num());

        // After a write cycle, further subsequent read transactions should also
        // succeed, but with a different sequence number.
        drop(lock.acquire());
        let token3 = lock.begin_read_transaction();
        assert!(lock.end_read_transaction(token3));
        assert_ne!(token1.seq_num(), token3.seq_num());
    }

    #[test]
    fn test_contested_read() {
        let lock: SeqLock = SeqLock::new();

        // Any write cycle which happens during a read should cause the read
        // transaction to fail.
        let token = lock.begin_read_transaction();

        // Note that to keep life simple, and single threaded, we go through the
        // write cycle on this thread.
        let guard = lock.acquire();
        assert_eq!(lock.seq_num(), 1);
        assert!(!lock.end_read_transaction(token));

        drop(guard);
        assert_eq!(lock.seq_num(), 2);
        assert!(!lock.end_read_transaction(token));
    }

    #[test]
    fn test_read_non_blocking() {
        let lock: SeqLock = SeqLock::new();

        // Trying to begin a read transaction when there is no write-cycle in flight
        // should always succeed, even with a timeout of zero.
        let token = lock.try_begin_read_transaction().expect("no write cycle is in flight");
        assert!(lock.end_read_transaction(token));

        // Attempting to start a transaction while a write cycle is in progress should
        // always fail.
        let guard = lock.acquire();
        assert!(lock.try_begin_read_transaction().is_none());

        drop(guard);
        let token = lock.try_begin_read_transaction().expect("no write cycle is in flight");
        assert!(lock.end_read_transaction(token));
    }

    #[test]
    fn test_invalid_token() {
        let lock: SeqLock = SeqLock::new();
        let token = ReadTransactionToken::new();
        assert!(write_in_flight(token.seq_num()));
        assert!(!lock.end_read_transaction(token));
    }

    #[test]
    fn test_read_transaction_helper() {
        use core::sync::atomic::AtomicU64;

        let lock: SeqLock = SeqLock::new();
        let payload = AtomicU64::new(0);

        {
            let _guard = lock.acquire();
            payload.store(42, Ordering::Relaxed);
        }

        assert_eq!(lock.read_transaction(|| payload.load(Ordering::Relaxed)), 42);
    }

    #[test]
    fn test_uncontested_write() {
        let lock: SeqLock = SeqLock::new();

        // This one seems pretty trivial.  As long as there is only one writer,
        // acquire operations should always immediately succeed (including the
        // non-blocking version).
        const TRIALS: u32 = 1000;
        for i in 0..TRIALS {
            assert_eq!(lock.seq_num(), i * 4);

            drop(lock.acquire());
            assert_eq!(lock.seq_num(), (i * 4) + 2);

            let guard = lock.try_acquire().expect("uncontested try_acquire must succeed");
            assert_eq!(lock.seq_num(), (i * 4) + 3);
            drop(guard);
            assert_eq!(lock.seq_num(), (i * 4) + 4);
        }
    }

    #[test]
    fn test_try_acquire_is_contested() {
        let lock: SeqLock = SeqLock::new();

        let guard = lock.try_acquire().expect("uncontested try_acquire must succeed");
        assert_eq!(lock.seq_num(), 1);

        assert!(lock.try_acquire().is_none());
        assert_eq!(lock.seq_num(), 1);

        drop(guard);
        assert_eq!(lock.seq_num(), 2);
    }

    #[test]
    fn test_contested_write() {
        use core::sync::atomic::AtomicU32;
        use std::sync::Arc;
        use std::thread;
        use std::time::Duration;

        // Simulate contention, then make sure that the non-blocking form of
        // acquire fails.
        //
        // Make a best-effort attempt to validate a normal acquire.
        //
        // Note that this can never be a conclusive test.  In addition to never being
        // able to absolutely guarantee that our test thread has actually started the
        // acquire operation after signaling to us that it has (via the shared state),
        // no matter how long we wait, we can never actually prove that it the test
        // thread _wouldn't_ have eventually entered the exclusive portion of the lock
        // had we simply waited a bit longer.
        const NOT_STARTED: u32 = 0;
        const ATTEMPTING_ACQUIRE: u32 = 1;
        const ACQUIRE_SUCCEEDED: u32 = 2;

        let lock = Arc::new(SeqLock::<SYNC_OPT_FENCE>::new());
        let state = Arc::new(AtomicU32::new(NOT_STARTED));

        let guard = lock.acquire();
        assert!(lock.try_acquire().is_none());

        let acquire_thread = {
            let lock = lock.clone();
            let state = state.clone();
            thread::spawn(move || {
                state.store(ATTEMPTING_ACQUIRE, Ordering::SeqCst);
                let guard = lock.acquire();
                state.store(ACQUIRE_SUCCEEDED, Ordering::SeqCst);
                drop(guard);
            })
        };

        // Wait forever for the thread start it's acquire attempt.
        while state.load(Ordering::SeqCst) != ATTEMPTING_ACQUIRE {
            // empty body.  Just spinning.
            arch_yield();
        }

        // Wait just a bit, then verify that the test thread has still not acquired
        // the lock.
        thread::sleep(Duration::from_millis(500));
        assert_eq!(state.load(Ordering::SeqCst), ATTEMPTING_ACQUIRE);

        // Release the lock and verify that the test thread successfully acquires and
        // release it.
        drop(guard);
        while state.load(Ordering::SeqCst) != ACQUIRE_SUCCEEDED {
            // empty body.  Just spinning.
            arch_yield();
        }

        // We should now be able to bounce through the lock without any significant
        // delay. The test thread may still be in the process of releasing the lock,
        // but it should eventually succeed.
        drop(lock.acquire());

        // The acquire_thread may not have exited yet, but it should do so in short
        // order.
        acquire_thread.join().unwrap();
    }

    #[test]
    fn test_concurrent_readers_writers() {
        use core::sync::atomic::AtomicU64;
        use std::sync::Arc;
        use std::thread;

        // `b` is always the complement of `a`, so a torn read is detectable.
        struct Payload {
            lock: SeqLock,
            a: AtomicU64,
            b: AtomicU64,
        }

        const ITERATIONS: u64 = 5000;

        let payload =
            Arc::new(Payload { lock: SeqLock::new(), a: AtomicU64::new(0), b: AtomicU64::new(!0) });

        let writer = {
            let payload = payload.clone();
            thread::spawn(move || {
                for i in 1..=ITERATIONS {
                    let _guard = payload.lock.acquire();
                    payload.a.store(i, Ordering::Relaxed);
                    payload.b.store(!i, Ordering::Relaxed);
                }
            })
        };

        let mut successful_transactions: u64 = 0;
        let mut observed: u64 = 0;
        while observed < ITERATIONS {
            let token = payload.lock.begin_read_transaction();
            let a = payload.a.load(Ordering::Relaxed);
            let b = payload.b.load(Ordering::Relaxed);
            if payload.lock.end_read_transaction(token) {
                assert_eq!(a, !b, "observed a torn payload in a successful transaction");
                successful_transactions += 1;
                observed = observed.max(a);
            }
        }

        let (a, b) = payload.lock.read_transaction(|| {
            (payload.a.load(Ordering::Relaxed), payload.b.load(Ordering::Relaxed))
        });
        assert_eq!(a, !b);

        writer.join().unwrap();
        assert!(successful_transactions > 0, "no read transaction ever succeeded");
    }
}
