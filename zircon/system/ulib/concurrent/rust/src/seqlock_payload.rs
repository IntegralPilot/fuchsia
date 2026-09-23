// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zerocopy::{FromBytes, Immutable, IntoBytes};

use crate::common::{SYNC_OPT_ACQ_REL_OPS, SYNC_OPT_FENCE, SYNC_OPT_NONE, SyncOpt};
use crate::copy::WellDefinedCopyable;
use crate::seqlock::{SeqLock, WriteGuard};

/// Payload wrapper for data protected by a [`SeqLock`].
///
/// `SeqLockPayload<T, LOCK_SYNC_OPT>` wraps a [`WellDefinedCopyable<T>`] and
/// binds its synchronization mode (`COPY_SYNC_OPT`) to that required by
/// `SeqLock<LOCK_SYNC_OPT>`.
#[repr(transparent)]
pub struct SeqLockPayload<
    T: Copy + FromBytes + IntoBytes + Immutable,
    const LOCK_SYNC_OPT: u8 = SYNC_OPT_FENCE,
> {
    // `Clone` and `Copy` are intentionally not implemented so shared instances
    // cannot be copied non-atomically.
    payload: WellDefinedCopyable<T>,
}

impl<T: Copy + FromBytes + IntoBytes + Immutable, const LOCK_SYNC_OPT: u8>
    SeqLockPayload<T, LOCK_SYNC_OPT>
{
    /// The sync options we use for this payload are specified for us based on the
    /// lock type we plan to use this payload with.
    pub const COPY_SYNC_OPT: SyncOpt = SeqLock::<LOCK_SYNC_OPT>::COPY_WRAPPER_SYNC_OPT;

    /// Forwarding constructor for our payload.
    #[inline]
    pub const fn new(instance: T) -> Self {
        // The payload sync option selected by the lock should always be AcqRelOps,
        // or None (in the case where the lock is using fences). Assert this at
        // compile time.
        const {
            assert!(matches!(Self::COPY_SYNC_OPT, SyncOpt::AcqRelOps | SyncOpt::None));
        };
        Self { payload: WellDefinedCopyable::new(instance) }
    }

    /// Specific version of `read` which always uses the sync-opt dictated to
    /// us by our associated lock type.
    #[inline]
    pub fn read(&self, dst: &mut T) {
        if const { matches!(Self::COPY_SYNC_OPT, SyncOpt::AcqRelOps) } {
            self.payload.read::<SYNC_OPT_ACQ_REL_OPS>(dst);
        } else {
            self.payload.read::<SYNC_OPT_NONE>(dst);
        }
    }

    /// Specific version of `update` which always uses the sync-opt dictated to
    /// us by our associated lock type.
    #[inline]
    pub fn update(&self, _guard: &WriteGuard<'_, LOCK_SYNC_OPT>, src: &T) {
        if const { matches!(Self::COPY_SYNC_OPT, SyncOpt::AcqRelOps) } {
            self.payload.update::<SYNC_OPT_ACQ_REL_OPS>(src);
        } else {
            self.payload.update::<SYNC_OPT_NONE>(src);
        }
    }

    /// Gain R/W access to the payload in order to perform an in-place update of
    /// the contents using fence-to-fence synchronization to protect the payload.
    /// All stores to the payload accessed via this pointer should be done using
    /// relaxed atomic stores.
    #[inline]
    #[must_use]
    pub fn begin_in_place_update(&self, _guard: &WriteGuard<'_, LOCK_SYNC_OPT>) -> *mut T {
        const {
            assert!(
                LOCK_SYNC_OPT == SYNC_OPT_FENCE,
                "In-place updates can only be performed when using fence synchronization"
            );
        };
        self.payload.instance.get()
    }

    /// Exposes [`WellDefinedCopyable::unsynchronized_get`] for this payload.
    ///
    /// Dereferencing the returned pointer is only safe if `_guard` belongs to the
    /// [`SeqLock`] protecting this payload so that no concurrent writes can occur
    /// while reading the instance.
    #[inline]
    #[must_use]
    pub const fn unsynchronized_get(&self, _guard: &WriteGuard<'_, LOCK_SYNC_OPT>) -> *const T {
        self.payload.unsynchronized_get()
    }
}

impl<T: Copy + FromBytes + IntoBytes + Immutable + Default, const LOCK_SYNC_OPT: u8> Default
    for SeqLockPayload<T, LOCK_SYNC_OPT>
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

zr::static_assert!(size_of::<SeqLockPayload<u64>>() == size_of::<u64>());
zr::static_assert!(align_of::<SeqLockPayload<u64>>() == align_of::<u64>());
zr::static_assert!(size_of::<SeqLockPayload<[u64; 8]>>() == 64);
zr::static_assert!(align_of::<SeqLockPayload<[u64; 8]>>() == align_of::<u64>());
zr::static_assert!(
    size_of::<SeqLockPayload<[u64; 3], SYNC_OPT_ACQ_REL_OPS>>() == size_of::<[u64; 3]>()
);
zr::static_assert!(
    align_of::<SeqLockPayload<[u64; 3], SYNC_OPT_ACQ_REL_OPS>>() == align_of::<[u64; 3]>()
);

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, FromBytes, IntoBytes, Immutable)]
    struct TestTransform {
        a_offset: i64,
        b_offset: i64,
        numerator: u32,
        denominator: u32,
    }

    impl TestTransform {
        fn from_generation(generation: u64) -> Self {
            Self {
                a_offset: generation as i64,
                b_offset: !(generation as i64),
                numerator: generation as u32,
                denominator: !(generation as u32),
            }
        }

        fn is_consistent(&self) -> bool {
            (self.a_offset == !self.b_offset)
                && (self.numerator == !self.denominator)
                && (self.numerator == self.a_offset as u32)
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, FromBytes, IntoBytes, Immutable)]
    struct TestParams {
        values: [u64; 8],
    }

    impl TestParams {
        fn from_generation(generation: u64) -> Self {
            let mut values = [0; 8];
            for (i, value) in values.iter_mut().enumerate() {
                *value = generation.wrapping_add(i as u64);
            }
            Self { values }
        }

        fn is_consistent(&self) -> bool {
            self.values
                .iter()
                .enumerate()
                .all(|(i, value)| *value == self.values[0].wrapping_add(i as u64))
        }
    }

    zr::static_assert!(size_of::<TestTransform>() == 24);
    zr::static_assert!(size_of::<TestParams>() == 64);

    type AcqRelPayload<T> = SeqLockPayload<T, SYNC_OPT_ACQ_REL_OPS>;

    #[test]
    fn test_copy_sync_opt() {
        assert_eq!(SeqLockPayload::<u64>::COPY_SYNC_OPT, SyncOpt::None);
        assert_eq!(AcqRelPayload::<u64>::COPY_SYNC_OPT, SyncOpt::AcqRelOps);

        assert_eq!(
            SeqLockPayload::<u64>::COPY_SYNC_OPT,
            SeqLock::<SYNC_OPT_FENCE>::COPY_WRAPPER_SYNC_OPT
        );
        assert_eq!(
            AcqRelPayload::<u64>::COPY_SYNC_OPT,
            SeqLock::<SYNC_OPT_ACQ_REL_OPS>::COPY_WRAPPER_SYNC_OPT
        );
    }

    #[test]
    fn test_layout() {
        assert_eq!(size_of::<SeqLockPayload<TestTransform>>(), size_of::<TestTransform>());
        assert_eq!(align_of::<SeqLockPayload<TestTransform>>(), align_of::<TestTransform>());
        assert_eq!(size_of::<SeqLockPayload<TestParams>>(), size_of::<TestParams>());
        assert_eq!(align_of::<SeqLockPayload<TestParams>>(), align_of::<TestParams>());
    }

    #[test]
    fn test_read_update_single_threaded() {
        let lock: SeqLock = SeqLock::new();
        let payload = SeqLockPayload::<TestTransform>::default();

        let mut dst = TestTransform::from_generation(99);
        payload.read(&mut dst);
        assert_eq!(dst, TestTransform::default());

        let expected = TestTransform::from_generation(42);
        {
            let guard = lock.acquire();
            payload.update(&guard, &expected);

            // SAFETY: The guard belongs to the lock protecting `payload`,
            // and there is no concurrent writer.
            assert_eq!(unsafe { *payload.unsynchronized_get(&guard) }, expected);
        }

        let mut observed = TestTransform::default();
        lock.read_transaction(|| payload.read(&mut observed));
        assert_eq!(observed, expected);
    }

    #[test]
    fn test_read_update_acq_rel() {
        let lock = SeqLock::<SYNC_OPT_ACQ_REL_OPS>::new();
        let payload = AcqRelPayload::<TestParams>::new(TestParams::from_generation(1));

        let mut dst = TestParams::default();
        payload.read(&mut dst);
        assert_eq!(dst, TestParams::from_generation(1));

        let expected = TestParams::from_generation(9999);
        {
            let guard = lock.acquire();
            payload.update(&guard, &expected);
        }

        let mut observed = TestParams::default();
        lock.read_transaction(|| payload.read(&mut observed));
        assert_eq!(observed, expected);
    }

    #[test]
    fn test_in_place_update() {
        use core::sync::atomic::{AtomicI64, Ordering};

        let lock: SeqLock = SeqLock::new();
        let payload = SeqLockPayload::<TestTransform>::default();

        {
            let guard = lock.acquire();
            let ptr = payload.begin_in_place_update(&guard);

            // SAFETY: `ptr` points to the live payload, `a_offset` is aligned for
            // `AtomicI64`, and the payload is only accessed atomically.
            unsafe {
                AtomicI64::from_ptr(core::ptr::addr_of_mut!((*ptr).a_offset))
                    .store(1234, Ordering::Relaxed);
            }
        }

        let mut observed = TestTransform::default();
        lock.read_transaction(|| payload.read(&mut observed));
        assert_eq!(observed.a_offset, 1234);
    }

    #[test]
    fn test_concurrent_readers_writers() {
        use std::sync::Arc;
        use std::thread;

        struct Shared {
            lock: SeqLock,
            transform: SeqLockPayload<TestTransform>,
            params: SeqLockPayload<TestParams>,
        }

        const ITERATIONS: u64 = 5000;

        let shared = Arc::new(Shared {
            lock: SeqLock::new(),
            transform: SeqLockPayload::new(TestTransform::from_generation(0)),
            params: SeqLockPayload::new(TestParams::from_generation(0)),
        });

        let writer = {
            let shared = shared.clone();
            thread::spawn(move || {
                for i in 1..=ITERATIONS {
                    let guard = shared.lock.acquire();
                    shared.transform.update(&guard, &TestTransform::from_generation(i));
                    shared.params.update(&guard, &TestParams::from_generation(i));
                }
            })
        };

        let mut successful_transactions: u64 = 0;
        let mut observed: u64 = 0;
        let mut transform = TestTransform::default();
        let mut params = TestParams::default();
        while observed < ITERATIONS {
            let token = shared.lock.begin_read_transaction();
            shared.transform.read(&mut transform);
            shared.params.read(&mut params);
            if shared.lock.end_read_transaction(token) {
                assert!(
                    transform.is_consistent(),
                    "observed a torn transform in a successful transaction: {transform:?}"
                );
                assert!(
                    params.is_consistent(),
                    "observed a torn parameter block in a successful transaction: {params:?}"
                );
                assert_eq!(
                    transform,
                    TestTransform::from_generation(params.values[0]),
                    "observed two payloads from different generations"
                );

                successful_transactions += 1;
                observed = observed.max(params.values[0]);
            }
        }

        shared.lock.read_transaction(|| {
            shared.transform.read(&mut transform);
            shared.params.read(&mut params);
        });
        assert!(transform.is_consistent());
        assert!(params.is_consistent());

        writer.join().unwrap();
        assert!(successful_transactions > 0, "no read transaction ever succeeded");
    }
}
