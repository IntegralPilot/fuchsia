// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::cell::UnsafeCell;
use core::sync::atomic::{Ordering, fence};
use zerocopy::{FromBytes, Immutable, IntoBytes};

use crate::common::{SYNC_OPT_FENCE, SYNC_OPT_NONE};

pub use internal::MAX_TRANSFER_GRANULARITY;

use internal::{COPY_DIR_FROM, COPY_DIR_TO, MaxTransferAligned, well_defined_copy};

mod internal {
    use core::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};

    use crate::common::{SYNC_OPT_ACQ_REL_OPS, SYNC_OPT_NONE};

    pub const MAX_TRANSFER_GRANULARITY: usize = size_of::<u64>();
    pub(super) const COPY_DIR_TO: u8 = 0;
    pub(super) const COPY_DIR_FROM: u8 = 1;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum MaxTransferAligned {
        No,
        Yes,
    }

    pub(super) trait TransferElement: Copy {
        /// # Safety
        ///
        /// `dst` must be valid for writes of `size_of::<Self>()` bytes, naturally
        /// aligned for `Self`, and not concurrently accessed with non-atomic operations.
        unsafe fn atomic_store(dst: *mut Self, val: Self, order: Ordering);

        /// # Safety
        ///
        /// `src` must be valid for reads of `size_of::<Self>()` bytes, naturally
        /// aligned for `Self`, and not concurrently accessed with non-atomic operations.
        unsafe fn atomic_load(src: *const Self, order: Ordering) -> Self;
    }

    macro_rules! impl_transfer_element {
        ($int:ty, $atomic:ty) => {
            impl TransferElement for $int {
                #[inline(always)]
                unsafe fn atomic_store(dst: *mut Self, val: Self, order: Ordering) {
                    // SAFETY: The caller guarantees `dst` is valid for writes, naturally
                    // aligned, and not concurrently accessed with non-atomic operations.
                    unsafe { <$atomic>::from_ptr(dst).store(val, order) }
                }

                #[inline(always)]
                unsafe fn atomic_load(src: *const Self, order: Ordering) -> Self {
                    // SAFETY: The caller guarantees `src` is valid for reads, naturally
                    // aligned, and not concurrently accessed with non-atomic operations.
                    unsafe { <$atomic>::from_ptr(src.cast_mut()).load(order) }
                }
            }
        };
    }

    impl_transfer_element!(u8, AtomicU8);
    impl_transfer_element!(u16, AtomicU16);
    impl_transfer_element!(u32, AtomicU32);
    impl_transfer_element!(u64, AtomicU64);

    #[inline(always)]
    pub(super) unsafe fn copy_element<T: TransferElement, const DIR: u8>(
        dst: *mut u8,
        src: *const u8,
        offset_bytes: usize,
        order: Ordering,
    ) {
        // SAFETY: The caller guarantees `src + offset_bytes` and `dst + offset_bytes`
        // remain in bounds and are naturally aligned for `T`.
        let src = unsafe { src.add(offset_bytes) }.cast::<T>();
        // SAFETY: As above.
        let dst = unsafe { dst.add(offset_bytes) }.cast::<T>();

        debug_assert!(
            ((DIR == COPY_DIR_TO) && matches!(order, Ordering::Relaxed | Ordering::Release))
                || ((DIR == COPY_DIR_FROM)
                    && matches!(order, Ordering::Relaxed | Ordering::Acquire))
        );

        if const { DIR == COPY_DIR_TO } {
            // SAFETY: The caller guarantees `dst` is valid for atomic stores and `src` is valid
            // for reads of `T`.
            unsafe { T::atomic_store(dst, *src, order) };
        } else {
            // SAFETY: The caller guarantees `src` is valid for atomic loads and `dst` is valid
            // for writes of `T`.
            unsafe { *dst = T::atomic_load(src, order) };
        }
    }

    #[inline]
    pub(super) unsafe fn well_defined_copy<const DIR: u8, const SYNC_OPT: u8>(
        dst: *mut u8,
        src: *const u8,
        size_bytes: usize,
        max_transfer_aligned: MaxTransferAligned,
    ) {
        // To keep life simple, we demand that both the source and the destination
        // have the same alignment relative to our max transfer granularity.
        debug_assert!(
            ((src as usize) & (MAX_TRANSFER_GRANULARITY - 1))
                == ((dst as usize) & (MAX_TRANSFER_GRANULARITY - 1)),
            "src {:p} dst {:p} granularity {}",
            src,
            dst,
            MAX_TRANSFER_GRANULARITY
        );

        // In debug builds, make sure that src and dst obey the specified
        // worst case alignment.
        //
        // TODO(johngro): Consider demoting these asserts to existing only in a
        // super-duper-debug build, if we ever have such a thing.
        debug_assert!(
            (max_transfer_aligned == MaxTransferAligned::No)
                || ((src as usize) & (MAX_TRANSFER_GRANULARITY - 1)) == 0
        );
        debug_assert!(
            (max_transfer_aligned == MaxTransferAligned::No)
                || ((dst as usize) & (MAX_TRANSFER_GRANULARITY - 1)) == 0
        );

        // Sync options at this point should be either to use Acquire/Release on the
        // options, or to simply use relaxed.  Use of fences should have been handled
        // at the inline wrapper level.
        assert!((SYNC_OPT == SYNC_OPT_ACQ_REL_OPS) || (SYNC_OPT == SYNC_OPT_NONE));

        if size_bytes == 0 {
            return;
        }

        let order = if SYNC_OPT == SYNC_OPT_NONE {
            Ordering::Relaxed
        } else if DIR == COPY_DIR_TO {
            Ordering::Release
        } else {
            Ordering::Acquire
        };

        // Start by bringing our pointer to 8 byte alignment.  Skip any steps which
        // are not required based on our specified worst case alignment.
        let mut offset_bytes: usize = 0;
        if max_transfer_aligned == MaxTransferAligned::No {
            if ((src as usize + offset_bytes) & 1) != 0 && (size_bytes - offset_bytes) >= 1 {
                // SAFETY: Both buffers are valid for `size_bytes` bytes and share the same
                // alignment relative to `MAX_TRANSFER_GRANULARITY`.
                unsafe { copy_element::<u8, DIR>(dst, src, offset_bytes, order) };
                offset_bytes += 1;
            }
            if ((src as usize + offset_bytes) & 2) != 0 && (size_bytes - offset_bytes) >= 2 {
                // SAFETY: Both buffers are valid for `size_bytes` bytes and 2-byte aligned
                // at `offset_bytes`.
                unsafe { copy_element::<u16, DIR>(dst, src, offset_bytes, order) };
                offset_bytes += 2;
            }
            if ((src as usize + offset_bytes) & 4) != 0 && (size_bytes - offset_bytes) >= 4 {
                // SAFETY: Both buffers are valid for `size_bytes` bytes and 4-byte aligned
                // at `offset_bytes`.
                unsafe { copy_element::<u32, DIR>(dst, src, offset_bytes, order) };
                offset_bytes += 4;
            }
        }

        // Now copy the bulk portion of the data using 64 bit transfers.
        const { assert!(MAX_TRANSFER_GRANULARITY == size_of::<u64>()) };
        while offset_bytes + size_of::<u64>() <= size_bytes {
            // SAFETY: Both buffers are valid for `size_bytes` bytes and 8-byte aligned
            // at `offset_bytes`.
            unsafe { copy_element::<u64, DIR>(dst, src, offset_bytes, order) };
            offset_bytes += size_of::<u64>();
        }

        // If there is anything left to do, take care of the remainder using smaller
        // transfers.
        let remainder = size_bytes - offset_bytes;
        if remainder > 0 {
            // SAFETY: Both buffers are valid for `size_bytes` bytes and sufficiently aligned
            // at `offset_bytes` for each transfer width.
            unsafe {
                match remainder {
                    1 => copy_element::<u8, DIR>(dst, src, offset_bytes, order),
                    2 => copy_element::<u16, DIR>(dst, src, offset_bytes, order),
                    3 => {
                        copy_element::<u16, DIR>(dst, src, offset_bytes, order);
                        copy_element::<u8, DIR>(dst, src, offset_bytes + 2, order);
                    }
                    4 => copy_element::<u32, DIR>(dst, src, offset_bytes, order),
                    5 => {
                        copy_element::<u32, DIR>(dst, src, offset_bytes, order);
                        copy_element::<u8, DIR>(dst, src, offset_bytes + 4, order);
                    }
                    6 => {
                        copy_element::<u32, DIR>(dst, src, offset_bytes, order);
                        copy_element::<u16, DIR>(dst, src, offset_bytes + 4, order);
                    }
                    7 => {
                        copy_element::<u32, DIR>(dst, src, offset_bytes, order);
                        copy_element::<u16, DIR>(dst, src, offset_bytes + 4, order);
                        copy_element::<u8, DIR>(dst, src, offset_bytes + 6, order);
                    }
                    _ => debug_assert!(false),
                }
            }
        }
    }
}

/// Copy `size_bytes` bytes from `src` to `dst` using atomic store operations to move
/// the element into `dst` so that the behavior of the system is always well
/// defined, even if there is a `well_defined_copy_from` operation reading from the
/// memory pointed to by `dst` concurrent with this `well_defined_copy_to` operation.
///
/// `well_defined_copy_to` has `memcpy` semantics, not `memmove` semantics. In other
/// words, it is illegal for `src` or `dst` to overlap in any way.
///
/// While it is not required, by default, that `src` and `dst` have any specific
/// alignment, both `src` and `dst` *must* have the _same_ alignment.
///
/// IOW: (`src` & 0x7) *must* equal (`dst` & 0x7)
///
/// # Const Generic Args
///
/// `SYNC_OPT`
/// Controls the options for memory order synchronization. See the comments on
/// [`crate::common::SyncOpt`] for details.
///
/// `WORST_CASE_ALIGNMENT`
/// An explicit guarantee of the worst case alignment that `src`/`dst` will obey.
/// When this alignment guarantee is greater than or equal to the maximum
/// internal transfer granularity of 64 bits, the initial explicit alignment step
/// of the operation can be optimized away for a minor performance gain.
///
/// # Safety
///
/// * `src` and `dst` must be valid for reads and writes (respectively) of `size_bytes`
///   bytes, must not overlap, and must have the same alignment modulo 8.
/// * Both pointers must be aligned to at least `WORST_CASE_ALIGNMENT`.
/// * `dst` must only be accessed concurrently through well-defined copy operations,
///   and `src` must not be concurrently mutated.
#[inline]
pub unsafe fn well_defined_copy_to<const SYNC_OPT: u8, const WORST_CASE_ALIGNMENT: usize>(
    dst: *mut u8,
    src: *const u8,
    size_bytes: usize,
) {
    const {
        assert!(
            WORST_CASE_ALIGNMENT.is_power_of_two(),
            "WORST_CASE_ALIGNMENT must be a power of 2"
        );
    };
    let mta = if WORST_CASE_ALIGNMENT >= MAX_TRANSFER_GRANULARITY {
        MaxTransferAligned::Yes
    } else {
        MaxTransferAligned::No
    };

    if const { SYNC_OPT == SYNC_OPT_FENCE } {
        fence(Ordering::Release);
        // SAFETY: The caller guarantees the safety preconditions of `well_defined_copy`.
        unsafe { well_defined_copy::<COPY_DIR_TO, SYNC_OPT_NONE>(dst, src, size_bytes, mta) };
    } else {
        // SAFETY: The caller guarantees the safety preconditions of `well_defined_copy`.
        unsafe { well_defined_copy::<COPY_DIR_TO, SYNC_OPT>(dst, src, size_bytes, mta) };
    }
}

/// Copy `size_bytes` bytes from `src` to `dst` using atomic load operations to load the
/// element from `src` so that the behavior of the system is always well defined,
/// even if there is a `well_defined_copy_to` operation writing to the memory pointed
/// to by `src` concurrent with this `well_defined_copy_from` operation.
///
/// `well_defined_copy_from` has `memcpy` semantics, not `memmove` semantics. In other
/// words, it is illegal for `src` or `dst` to overlap in any way.
///
/// While it is not required, by default, that `src` and `dst` have any specific
/// alignment, both `src` and `dst` *must* have the _same_ alignment.
///
/// IOW: (`src` & 0x7) *must* equal (`dst` & 0x7)
///
/// # Const Generic Args
///
/// `SYNC_OPT`
/// Controls the options for memory order synchronization. See the comments on
/// [`crate::common::SyncOpt`] for details.
///
/// `WORST_CASE_ALIGNMENT`
/// An explicit guarantee of the worst case alignment that `src`/`dst` will obey.
/// When this alignment guarantee is greater than or equal to the maximum
/// internal transfer granularity of 64 bits, the initial explicit alignment step
/// of the operation can be optimized away for a minor performance gain.
///
/// # Safety
///
/// * `src` and `dst` must be valid for reads and writes (respectively) of `size_bytes`
///   bytes, must not overlap, and must have the same alignment modulo 8.
/// * Both pointers must be aligned to at least `WORST_CASE_ALIGNMENT`.
/// * `src` must only be accessed concurrently through well-defined copy operations,
///   and `dst` must not be concurrently accessed.
#[inline]
pub unsafe fn well_defined_copy_from<const SYNC_OPT: u8, const WORST_CASE_ALIGNMENT: usize>(
    dst: *mut u8,
    src: *const u8,
    size_bytes: usize,
) {
    const {
        assert!(
            WORST_CASE_ALIGNMENT.is_power_of_two(),
            "WORST_CASE_ALIGNMENT must be a power of 2"
        );
    };
    let mta = if WORST_CASE_ALIGNMENT >= MAX_TRANSFER_GRANULARITY {
        MaxTransferAligned::Yes
    } else {
        MaxTransferAligned::No
    };

    if const { SYNC_OPT == SYNC_OPT_FENCE } {
        // SAFETY: The caller guarantees the safety preconditions of `well_defined_copy`.
        unsafe { well_defined_copy::<COPY_DIR_FROM, SYNC_OPT_NONE>(dst, src, size_bytes, mta) };
        fence(Ordering::Acquire);
    } else {
        // SAFETY: The caller guarantees the safety preconditions of `well_defined_copy`.
        unsafe { well_defined_copy::<COPY_DIR_FROM, SYNC_OPT>(dst, src, size_bytes, mta) };
    }
}

/// Wrapper for transferring trivially copyable data into and out of shared
/// memory using well-defined atomic operations.
///
/// Users wrap a type `T` in `WellDefinedCopyable<T>` and use [`Self::update`]
/// and [`Self::read`] to copy data into and out of the contained `T` instance,
/// respectively. These methods deliberately restrict access to the underlying
/// storage so transfers occur through the lowest-level well-defined copy
/// functions.
///
/// `T` must implement [`Copy`], [`FromBytes`], [`IntoBytes`], and [`Immutable`]
/// to guarantee that it has no uninitialized padding bytes and that any byte
/// pattern observed during a concurrent transfer is a valid representation of
/// `T`. In addition, `align_of::<T>()` must be at least
/// [`MAX_TRANSFER_GRANULARITY`] (8 bytes) so that source and destination
/// buffers always share identical alignment modulo 8.
#[repr(transparent)]
pub struct WellDefinedCopyable<T: Copy + FromBytes + IntoBytes + Immutable> {
    // `Clone` and `Copy` are intentionally not implemented so shared instances
    // cannot be copied non-atomically.
    pub(crate) instance: UnsafeCell<T>,
}

// SAFETY: Every concurrent access to `instance` is performed using atomic operations,
// and `T: Send` allows values to be transferred across threads.
unsafe impl<T: Copy + FromBytes + IntoBytes + Immutable + Send> Sync for WellDefinedCopyable<T> {}

impl<T: Copy + FromBytes + IntoBytes + Immutable> WellDefinedCopyable<T> {
    const VALID_TYPE: () = {
        assert!(
            size_of::<Self>() == size_of::<T>(),
            "WellDefinedCopyable<T> must be the same size as T"
        );
        assert!(
            align_of::<Self>() == align_of::<T>(),
            "WellDefinedCopyable<T> must have the same alignment as T"
        );
        assert!(
            align_of::<T>() >= MAX_TRANSFER_GRANULARITY,
            "T must have alignment >= MAX_TRANSFER_GRANULARITY"
        );
    };

    /// Creates a new `WellDefinedCopyable` wrapping `instance`.
    #[inline]
    pub const fn new(instance: T) -> Self {
        let () = Self::VALID_TYPE;
        Self { instance: UnsafeCell::new(instance) }
    }

    /// Read from the wrapped object into the destination buffer provided by the caller.
    #[inline]
    pub fn read<const SYNC_OPT: u8>(&self, dst: &mut T) {
        let () = Self::VALID_TYPE;
        // SAFETY: `dst` and `self.instance` are valid for `size_of::<T>()` bytes,
        // non-overlapping, aligned to at least `MAX_TRANSFER_GRANULARITY` (and thus
        // share the same alignment modulo 8), and `self.instance` is only accessed
        // atomically.
        unsafe {
            well_defined_copy_from::<SYNC_OPT, MAX_TRANSFER_GRANULARITY>(
                core::ptr::from_mut(dst).cast::<u8>(),
                self.instance.get().cast_const().cast::<u8>(),
                size_of::<T>(),
            );
        }
    }

    /// Update the wrapped object from the source buffer provided by the caller.
    #[inline]
    pub fn update<const SYNC_OPT: u8>(&self, src: &T) {
        let () = Self::VALID_TYPE;
        // SAFETY: `self.instance` and `src` are valid for `size_of::<T>()` bytes,
        // non-overlapping, aligned to at least `MAX_TRANSFER_GRANULARITY` (and thus
        // share the same alignment modulo 8), and `self.instance` is only accessed
        // atomically.
        unsafe {
            well_defined_copy_to::<SYNC_OPT, MAX_TRANSFER_GRANULARITY>(
                self.instance.get().cast::<u8>(),
                core::ptr::from_ref(src).cast::<u8>(),
                size_of::<T>(),
            );
        }
    }

    /// WARNING: There be dragons here!
    ///
    /// `unsynchronized_get` returns a raw pointer providing direct read-only
    /// access to the underlying instance of `T`. Dereferencing the pointer is
    /// _only_ safe if the user can guarantee that no write operations may be
    /// concurrently performed against the storage while reading the instance.
    ///
    /// One example of a legitimate use of this method might be when a user is
    /// operating in the write exclusive portion of a sequence lock. They are
    /// guaranteed to be the only potential writer of the wrapped object, so while
    /// it is still important that they continue to use `update` when they wish to
    /// mutate their instance of `T`, it is OK for them to read `T` directly without
    /// using `read` as this will not cause any undefined behavior when done
    /// concurrently with other readers in the system.
    ///
    /// Returning a raw pointer `*const T` rather than a reference `&T` avoids
    /// Rust's aliasing requirement that the pointee remain immutable for the
    /// entire lifetime of a reference, matching C++ where holding a reference
    /// across concurrent writes is permitted as long as it is not read during a
    /// write.
    #[inline]
    #[must_use]
    pub const fn unsynchronized_get(&self) -> *const T {
        self.instance.get().cast_const()
    }
}

impl<T: Copy + FromBytes + IntoBytes + Immutable + Default> Default for WellDefinedCopyable<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

zr::static_assert!(size_of::<WellDefinedCopyable<u64>>() == size_of::<u64>());
zr::static_assert!(align_of::<WellDefinedCopyable<u64>>() == align_of::<u64>());
zr::static_assert!(size_of::<WellDefinedCopyable<[u64; 8]>>() == 64);
zr::static_assert!(align_of::<WellDefinedCopyable<[u64; 8]>>() == align_of::<u64>());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::SYNC_OPT_ACQ_REL_OPS;

    const TEST_BUFFER_SIZE: usize = 256;

    trait TestObj:
        Copy + Default + core::fmt::Debug + PartialEq + Eq + FromBytes + IntoBytes + Immutable
    {
        type Inner: Copy + Default + core::fmt::Debug + PartialEq + Eq;
        fn new(val: Self::Inner) -> Self;
        fn val(&self) -> Self::Inner;
    }

    macro_rules! define_simple_obj {
        ($name:ident, $inner:ty, $pad_len:expr) => {
            #[repr(C, align(8))]
            #[derive(
                Clone, Copy, Debug, Default, PartialEq, Eq, FromBytes, IntoBytes, Immutable,
            )]
            struct $name {
                val: $inner,
                _pad: [u8; $pad_len],
            }

            impl TestObj for $name {
                type Inner = $inner;
                fn new(val: $inner) -> Self {
                    Self { val, _pad: [0; $pad_len] }
                }
                fn val(&self) -> $inner {
                    self.val
                }
            }
        };
    }

    define_simple_obj!(SimpleObjU8, u8, 7);
    define_simple_obj!(SimpleObjU16, u16, 6);
    define_simple_obj!(SimpleObjU32, u32, 4);
    define_simple_obj!(SimpleObjU64, u64, 0);

    #[repr(align(8))]
    struct TestBuffer([u8; TEST_BUFFER_SIZE]);

    impl TestBuffer {
        const fn new() -> Self {
            Self([0; TEST_BUFFER_SIZE])
        }
    }

    struct Rng(u64);

    impl Rng {
        const CONST_SEED: u64 = 0xa5f0_84a2_c3de_6b75;

        const fn new() -> Self {
            Self(Self::CONST_SEED)
        }

        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn next_u8(&mut self) -> u8 {
            self.next() as u8
        }
    }

    struct ConcurrentCopyFixture {
        src: TestBuffer,
        dst: TestBuffer,
        generator: Rng,
    }

    impl ConcurrentCopyFixture {
        fn new() -> Self {
            let mut fixture =
                Self { src: TestBuffer::new(), dst: TestBuffer::new(), generator: Rng::new() };
            fixture.reset_buffer();
            fixture
        }

        fn reset_buffer(&mut self) {
            for i in 0..TEST_BUFFER_SIZE {
                self.dst.0[i] = self.generator.next_u8();
                self.src.0[i] = !self.dst.0[i];
            }
        }

        fn do_wrapper_copy_test<O: TestObj, const SYNC_OPT: u8>(val: O::Inner) {
            let wrapped = WellDefinedCopyable::<O>::default();
            {
                let unwrapped = O::new(val);
                assert_eq!(val, unwrapped.val());
                // SAFETY: Single-threaded test with exclusive access to `wrapped`.
                assert_eq!(O::Inner::default(), unsafe { (*wrapped.unsynchronized_get()).val() });

                wrapped.update::<SYNC_OPT>(&unwrapped);

                assert_eq!(val, unwrapped.val());
                // SAFETY: Single-threaded test with exclusive access to `wrapped`.
                assert_eq!(val, unsafe { (*wrapped.unsynchronized_get()).val() });
            }

            {
                let mut unwrapped = O::default();
                assert_eq!(O::Inner::default(), unwrapped.val());
                // SAFETY: Single-threaded test with exclusive access to `wrapped`.
                assert_eq!(val, unsafe { (*wrapped.unsynchronized_get()).val() });

                wrapped.read::<SYNC_OPT>(&mut unwrapped);

                assert_eq!(val, unwrapped.val());
                // SAFETY: Single-threaded test with exclusive access to `wrapped`.
                assert_eq!(val, unsafe { (*wrapped.unsynchronized_get()).val() });
            }
        }

        fn do_wrapper_test<O: TestObj>(val: O::Inner) {
            // Default Construction
            {
                let wrapped = WellDefinedCopyable::<O>::default();
                // SAFETY: Single-threaded test with exclusive access to `wrapped`.
                assert_eq!(O::Inner::default(), unsafe { (*wrapped.unsynchronized_get()).val() });
            }

            // Explicit Construction
            {
                let wrapped = WellDefinedCopyable::new(O::new(val));
                // SAFETY: Single-threaded test with exclusive access to `wrapped`.
                assert_eq!(val, unsafe { (*wrapped.unsynchronized_get()).val() });
            }

            // Copy with various sync options.
            Self::do_wrapper_copy_test::<O, SYNC_OPT_ACQ_REL_OPS>(val);
            Self::do_wrapper_copy_test::<O, SYNC_OPT_FENCE>(val);
            Self::do_wrapper_copy_test::<O, SYNC_OPT_NONE>(val);
        }
    }

    #[test]
    fn test_copy_to() {
        let mut fixture = ConcurrentCopyFixture::new();

        assert_eq!(fixture.src.0.len(), fixture.dst.0.len());

        // Test all of the combinations of alignment at the start and end of the operation.
        for offset in 0..size_of::<u64>() {
            for remainder in 1..=size_of::<u64>() {
                let op_len = fixture.src.0.len() - offset - (size_of::<u64>() - remainder);

                assert!(op_len + offset <= fixture.src.0.len());

                // Perform a copy-to using release semantics for each element transfer,
                // and no fence, then check the results.
                fixture.reset_buffer();
                // SAFETY: `src` and `dst` are distinct, valid for `op_len` bytes at `offset`,
                // and share the same alignment modulo 8.
                unsafe {
                    well_defined_copy_to::<SYNC_OPT_ACQ_REL_OPS, 1>(
                        fixture.dst.0.as_mut_ptr().add(offset),
                        fixture.src.0.as_ptr().add(offset),
                        op_len,
                    );
                }
                assert_eq!(
                    &fixture.dst.0[offset..offset + op_len],
                    &fixture.src.0[offset..offset + op_len]
                );

                // Same test, but this time use a release fence at the start of the
                // operation, and relaxed atomic semantics on the individual element
                // transfers.
                fixture.reset_buffer();
                // SAFETY: As above.
                unsafe {
                    well_defined_copy_to::<SYNC_OPT_FENCE, 1>(
                        fixture.dst.0.as_mut_ptr().add(offset),
                        fixture.src.0.as_ptr().add(offset),
                        op_len,
                    );
                }
                assert_eq!(
                    &fixture.dst.0[offset..offset + op_len],
                    &fixture.src.0[offset..offset + op_len]
                );

                // Same test, but this time do not use either a fence or release semantics
                // on each element. Instead, simply do everything with relaxed atomic
                // stores.
                fixture.reset_buffer();
                // SAFETY: As above.
                unsafe {
                    well_defined_copy_to::<SYNC_OPT_NONE, 1>(
                        fixture.dst.0.as_mut_ptr().add(offset),
                        fixture.src.0.as_ptr().add(offset),
                        op_len,
                    );
                }
                assert_eq!(
                    &fixture.dst.0[offset..offset + op_len],
                    &fixture.src.0[offset..offset + op_len]
                );
            }
        }

        // Finally, perform one more test using each of the fence options, but
        // guaranteeing that we have at least uint64_t alignment.
        assert_eq!(fixture.dst.0.as_ptr() as usize & (MAX_TRANSFER_GRANULARITY - 1), 0);
        assert_eq!(fixture.src.0.as_ptr() as usize & (MAX_TRANSFER_GRANULARITY - 1), 0);

        // Release on the ops.
        fixture.reset_buffer();
        // SAFETY: Both buffers are 8-byte aligned and valid for `TEST_BUFFER_SIZE` bytes.
        unsafe {
            well_defined_copy_to::<SYNC_OPT_ACQ_REL_OPS, MAX_TRANSFER_GRANULARITY>(
                fixture.dst.0.as_mut_ptr(),
                fixture.src.0.as_ptr(),
                fixture.dst.0.len(),
            );
        }
        assert_eq!(fixture.dst.0, fixture.src.0);

        // Use a release fence before the transfer.
        fixture.reset_buffer();
        // SAFETY: As above.
        unsafe {
            well_defined_copy_to::<SYNC_OPT_FENCE, MAX_TRANSFER_GRANULARITY>(
                fixture.dst.0.as_mut_ptr(),
                fixture.src.0.as_ptr(),
                fixture.dst.0.len(),
            );
        }
        assert_eq!(fixture.dst.0, fixture.src.0);

        // Relaxed atomics on the ops, no fence.
        fixture.reset_buffer();
        // SAFETY: As above.
        unsafe {
            well_defined_copy_to::<SYNC_OPT_NONE, MAX_TRANSFER_GRANULARITY>(
                fixture.dst.0.as_mut_ptr(),
                fixture.src.0.as_ptr(),
                fixture.dst.0.len(),
            );
        }
        assert_eq!(fixture.dst.0, fixture.src.0);
    }

    #[test]
    fn test_copy_from() {
        let mut fixture = ConcurrentCopyFixture::new();

        assert_eq!(fixture.src.0.len(), fixture.dst.0.len());

        // Test all of the combinations of alignment at the start and end of the operation.
        for offset in 0..size_of::<u64>() {
            for remainder in 1..=size_of::<u64>() {
                let op_len = fixture.src.0.len() - offset - (size_of::<u64>() - remainder);

                assert!(op_len + offset <= fixture.src.0.len());

                // Perform a copy-from using acquire semantics for each element transfer,
                // and no fence, then check the results.
                fixture.reset_buffer();
                // SAFETY: `src` and `dst` are distinct, valid for `op_len` bytes at `offset`,
                // and share the same alignment modulo 8.
                unsafe {
                    well_defined_copy_from::<SYNC_OPT_ACQ_REL_OPS, 1>(
                        fixture.dst.0.as_mut_ptr().add(offset),
                        fixture.src.0.as_ptr().add(offset),
                        op_len,
                    );
                }
                assert_eq!(
                    &fixture.dst.0[offset..offset + op_len],
                    &fixture.src.0[offset..offset + op_len]
                );

                // Same test, but this time use an acquire fence at the end of the
                // operation, and relaxed atomic semantics on the individual element
                // transfers.
                fixture.reset_buffer();
                // SAFETY: As above.
                unsafe {
                    well_defined_copy_from::<SYNC_OPT_FENCE, 1>(
                        fixture.dst.0.as_mut_ptr().add(offset),
                        fixture.src.0.as_ptr().add(offset),
                        op_len,
                    );
                }
                assert_eq!(
                    &fixture.dst.0[offset..offset + op_len],
                    &fixture.src.0[offset..offset + op_len]
                );

                // Same test, but this time do not use either a fence or acquire semantics
                // on each element. Instead, simply do everything with relaxed atomic
                // loads.
                fixture.reset_buffer();
                // SAFETY: As above.
                unsafe {
                    well_defined_copy_from::<SYNC_OPT_NONE, 1>(
                        fixture.dst.0.as_mut_ptr().add(offset),
                        fixture.src.0.as_ptr().add(offset),
                        op_len,
                    );
                }
                assert_eq!(
                    &fixture.dst.0[offset..offset + op_len],
                    &fixture.src.0[offset..offset + op_len]
                );
            }
        }

        // Finally, perform one more test using each of the fence options, but
        // guaranteeing that we have at least uint64_t alignment.
        assert_eq!(fixture.dst.0.as_ptr() as usize & (MAX_TRANSFER_GRANULARITY - 1), 0);
        assert_eq!(fixture.src.0.as_ptr() as usize & (MAX_TRANSFER_GRANULARITY - 1), 0);

        // Acquire on the ops.
        fixture.reset_buffer();
        // SAFETY: Both buffers are 8-byte aligned and valid for `TEST_BUFFER_SIZE` bytes.
        unsafe {
            well_defined_copy_from::<SYNC_OPT_ACQ_REL_OPS, MAX_TRANSFER_GRANULARITY>(
                fixture.dst.0.as_mut_ptr(),
                fixture.src.0.as_ptr(),
                fixture.dst.0.len(),
            );
        }
        assert_eq!(fixture.dst.0, fixture.src.0);

        // Use an acquire fence after the transfer.
        fixture.reset_buffer();
        // SAFETY: As above.
        unsafe {
            well_defined_copy_from::<SYNC_OPT_FENCE, MAX_TRANSFER_GRANULARITY>(
                fixture.dst.0.as_mut_ptr(),
                fixture.src.0.as_ptr(),
                fixture.dst.0.len(),
            );
        }
        assert_eq!(fixture.dst.0, fixture.src.0);

        // Relaxed atomics on the ops, no fence.
        fixture.reset_buffer();
        // SAFETY: As above.
        unsafe {
            well_defined_copy_from::<SYNC_OPT_NONE, MAX_TRANSFER_GRANULARITY>(
                fixture.dst.0.as_mut_ptr(),
                fixture.src.0.as_ptr(),
                fixture.dst.0.len(),
            );
        }
        assert_eq!(fixture.dst.0, fixture.src.0);
    }

    #[test]
    fn test_wrapper_copy() {
        ConcurrentCopyFixture::do_wrapper_test::<SimpleObjU8>(0xA5);
        ConcurrentCopyFixture::do_wrapper_test::<SimpleObjU16>(0xA55A);
        ConcurrentCopyFixture::do_wrapper_test::<SimpleObjU32>(0xA55A_1234);
        ConcurrentCopyFixture::do_wrapper_test::<SimpleObjU64>(0xA55A_1234_DEAD_BEEF);
    }

    #[test]
    fn test_small_copies() {
        let mut fixture = ConcurrentCopyFixture::new();

        // Test all combinations of starting alignment (0..8) and small transfer lengths
        // (0..=16), including short transfers that do not reach the next 8-byte boundary.
        for offset in 0..size_of::<u64>() {
            for op_len in 0..=16 {
                fixture.reset_buffer();
                let before = fixture.dst.0;

                // SAFETY: `src` and `dst` are distinct, valid for `op_len` bytes at `offset`,
                // and share the same alignment modulo 8.
                unsafe {
                    well_defined_copy_to::<SYNC_OPT_ACQ_REL_OPS, 1>(
                        fixture.dst.0.as_mut_ptr().add(offset),
                        fixture.src.0.as_ptr().add(offset),
                        op_len,
                    );
                }
                assert_eq!(
                    &fixture.dst.0[offset..offset + op_len],
                    &fixture.src.0[offset..offset + op_len]
                );
                assert_eq!(&fixture.dst.0[..offset], &before[..offset]);
                assert_eq!(&fixture.dst.0[offset + op_len..], &before[offset + op_len..]);

                fixture.reset_buffer();
                let before = fixture.dst.0;

                // SAFETY: As above.
                unsafe {
                    well_defined_copy_from::<SYNC_OPT_ACQ_REL_OPS, 1>(
                        fixture.dst.0.as_mut_ptr().add(offset),
                        fixture.src.0.as_ptr().add(offset),
                        op_len,
                    );
                }
                assert_eq!(
                    &fixture.dst.0[offset..offset + op_len],
                    &fixture.src.0[offset..offset + op_len]
                );
                assert_eq!(&fixture.dst.0[..offset], &before[..offset]);
                assert_eq!(&fixture.dst.0[offset + op_len..], &before[offset + op_len..]);
            }
        }
    }
}
