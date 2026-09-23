// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Support for implementing splittable MMIO regions.
//!
//! This module defines the [MmioRegion] type which provides safe [Mmio] and [MmioSplit]
//! implementations on top of the more relaxed [UnsafeMmio] trait.
//!
//! The [UnsafeMmio] trait allows mutations through a shared reference, provided the caller
//! ensures that store operations are not performed concurrently with any other operation that may
//! overlap it.
//!
//! Implementing [UnsafeMmio] correctly is likely to be simpler than implementing [Mmio] and
//! [MmioSplit] for many use cases.

use crate::{Mmio, MmioError, MmioExt, MmioSplit};
use core::borrow::Borrow;
use core::marker::PhantomData;
use core::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// An MMIO region that can be stored to through a shared reference.
///
/// This trait requires the caller to uphold some safety constraints, but enables a generic
/// implementation of [MmioSplit]. See the [MmioRegion] which provides a safe wrapper on top of
/// this trait.
///
/// This is primarily intended to simplify implementing the [MmioSplit] trait, not for users of the
/// library. However, it is possible to use [UnsafeMmio] directly, provided the safety requirements
/// are met.
///
/// # Safety
/// - Callers must ensure that stores are never performed concurrently with any other operation on
///   an overlapping range.
/// - Concurrent loads are allowed on overlapping ranges.
/// - Callers must ensure that offsets are suitably aligned for the type being loaded or stored.
pub trait UnsafeMmio {
    /// Returns the size, in bytes, of the underlying MMIO region that can be accessed through this
    /// object.
    fn len(&self) -> usize;

    /// Returns true if the MMIO region has a length of 0.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the first offset into this MMIO region that is suitably aligned for`align`.
    ///
    /// An offset is suitably aligned if `offset = align_offset(align) + i * align` for some `i`.
    fn align_offset(&self, align: usize) -> usize;

    /// Loads a u8 from this MMIO region at the given offset.
    ///
    /// # Safety
    /// See the trait-level documentation.
    unsafe fn load8_unchecked(&self, offset: usize) -> u8;

    /// Loads a u16 from this MMIO region at the given offset.
    ///
    /// # Safety
    /// See the trait-level documentation.
    unsafe fn load16_unchecked(&self, offset: usize) -> u16;

    /// Loads a u32 from this MMIO region at the given offset.
    ///
    /// # Safety
    /// See the trait-level documentation.
    unsafe fn load32_unchecked(&self, offset: usize) -> u32;

    /// Loads a u64 from this MMIO region at the given offset.
    ///
    /// # Safety
    /// See the trait-level documentation.
    unsafe fn load64_unchecked(&self, offset: usize) -> u64;

    /// Stores a u8 to this MMIO region at the given offset.
    ///
    /// # Safety
    /// See the trait-level documentation.
    unsafe fn store8_unchecked(&self, offset: usize, v: u8);

    /// Stores a u16 to this MMIO region at the given offset.
    ///
    /// # Safety
    /// See the trait-level documentation.
    unsafe fn store16_unchecked(&self, offset: usize, v: u16);

    /// Stores a u32 to this MMIO region at the given offset.
    ///
    /// # Safety
    /// See the trait-level documentation.
    unsafe fn store32_unchecked(&self, offset: usize, v: u32);

    /// Stores a u64 to this MMIO region at the given offset.
    ///
    /// # Safety
    /// See the trait-level documentation.
    unsafe fn store64_unchecked(&self, offset: usize, v: u64);

    /// Issues a memory write barrier.  It is guaranteed that all stores preceding this barrier will
    /// appear to have happened before all stores following this barrier.
    fn write_barrier(&self);
}

/// An `MmioRegion` provides a safe implementation of [Mmio] and [MmioSplit] on top of an
/// [UnsafeMmio] implementation.
///
/// The safety constraints of [UnsafeMmio] require callers to ensure that stores are not performed
/// concurrently with loads for any overlapping range.
///
/// This type meets these requirements while supporting being split into independently owned due to
/// the following:
///
/// 1. An MmioRegion has exclusive ownership of a sub-region from the wrapped [UnsafeMmio]
///    implementation (required by [MmioRegion::new]).
/// 2. An MmioRegion only performs operations that are fully contained within the region it owns.
/// 3. All stores are performed through a mutable reference (ensuring stores are exclusive with all
///    other operations to the owned region).
/// 4. When splitting off `MmioRegions`, the split_off region owns a range that was owned by region
///    it was split off from prior to the split, and that it has exclusive ownership of after the
///    split.
///
/// # Type Parameters
/// An MmioRegion is parameterized by two types:
/// - `Impl`: the [UnsafeMmio] implementation wrapped by this region.
/// - `Owner`: an object with shared ownership of the `Impl` instance.
///
/// An MmioRegion is splittable if `Owner` can be cloned.
pub struct MmioRegion<Impl, Owner = Impl> {
    owner: Owner,
    bounds: Range<usize>,
    phantom: PhantomData<Impl>,
}

impl<U: UnsafeMmio> MmioRegion<U> {
    /// Create a new `MmioRegion` that has exclusive ownership of the entire range.
    ///
    /// The returned object is guaranteed to be the only one capable of referencing any value in
    /// the range. It can be converted into one that can be split
    pub fn new(inner: U) -> Self {
        let bounds = 0..inner.len();
        let owner = inner;
        Self { owner, bounds, phantom: PhantomData }
    }

    /// Converts this region into one which can be split.
    pub fn into_split(self) -> MmioRegion<U, Rc<U>> {
        let owner = Rc::new(self.owner);
        let bounds = self.bounds;
        // Safety:
        // - this region exclusively owns its bounds.
        // - ownership of the UnsafeMmio is transferred into the Rc.
        // - the returned region has the same bounds as self did at the start of the call.
        unsafe { MmioRegion::<U, _>::new_unchecked(owner, bounds) }
    }

    /// Maps the inner type to a new type with `f`.
    pub fn map<O: UnsafeMmio, F: FnOnce(U) -> O>(self, f: F) -> MmioRegion<O> {
        MmioRegion::new(f(self.owner))
    }
}

impl<U: UnsafeMmio + Send + Sync> MmioRegion<U> {
    /// Converts this region into one which can be split and sent.
    pub fn into_split_send(self) -> MmioRegion<U, Arc<U>> {
        let owner = Arc::new(self.owner);
        let bounds = self.bounds;
        // Safety:
        // - this region exclusively owns its bounds.
        // - ownership of the UnsafeMmio is transferred into the Arc.
        // - the returned region has the same bounds as self did at the start of the call.
        unsafe { MmioRegion::<U, _>::new_unchecked(owner, bounds) }
    }
}

impl<Impl: UnsafeMmio, Owner: Borrow<Impl>> MmioRegion<Impl, Owner> {
    /// Create an MmioRegion that constrains all operations to the underlying wrapped UnsafeMmio
    /// to be within the given bounds.
    ///
    /// # Safety
    /// - For the lifetime of this MmioRegion or any split off from it the given range must only be
    ///   accessed through this MmioRegion or a region split off from it.
    unsafe fn new_unchecked(owner: Owner, bounds: Range<usize>) -> Self {
        Self { owner, bounds, phantom: PhantomData }
    }

    /// Resolves the offset relative to the start of this MmioRegion's bounds, provided that offset
    /// is suitably aligned for type T and there is sufficient capacity within this MmioRegion's
    /// bounds at the given offset.
    pub fn resolve_offset<T>(&self, offset: usize) -> Result<usize, MmioError> {
        self.check_suitable_for::<T>(offset)?;
        Ok(self.bounds.start + offset)
    }
}

impl<Impl: UnsafeMmio, Owner: Borrow<Impl>> MmioRegion<Impl, Owner> {
    /// Provides access to the wrapped [`UnsafeMmio`] instance.
    pub fn unsafe_mmio(&self) -> &Impl {
        self.owner.borrow()
    }
}

impl<Impl: UnsafeMmio, Owner: Borrow<Impl>> Mmio for MmioRegion<Impl, Owner> {
    fn len(&self) -> usize {
        self.bounds.len()
    }

    fn align_offset(&self, align: usize) -> usize {
        // Determine the first offset into the wrapped region that is correctly aligned.
        let first_aligned_offset = self.owner.borrow().align_offset(align);

        // An aligned offset is any where offset = first_aligned_offset + i * align.
        // Or where (offset - first_aligned_offset) % align = 0.
        //
        // For offsets relative to the start of this region, they are aligned if:
        // (rel_offset + region_start - first_aligned_offset) % align = 0.
        // or rel_offset % align = (first_aligned_offset - region_start) % align
        //
        // Therefore, the first aligned offset, relative to the start of this region, is:
        // (first_aligned_offset - region_start) % align.
        first_aligned_offset.wrapping_sub(self.bounds.start) % align
    }

    fn try_load8(&self, offset: usize) -> Result<u8, MmioError> {
        let offset = self.resolve_offset::<u8>(offset)?;
        // Safety:
        // - this region exclusively owns its covered range (required by safety constraints)
        // - the immutable receiver excludes stores for this entire range
        Ok(unsafe { self.owner.borrow().load8_unchecked(offset) })
    }

    fn try_load16(&self, offset: usize) -> Result<u16, MmioError> {
        let offset = self.resolve_offset::<u16>(offset)?;
        // Safety:
        // - this region exclusively owns its covered range (required by safety constraints)
        // - the immutable receiver excludes stores for this entire range
        Ok(unsafe { self.owner.borrow().load16_unchecked(offset) })
    }

    fn try_load32(&self, offset: usize) -> Result<u32, MmioError> {
        let offset = self.resolve_offset::<u32>(offset)?;
        // Safety:
        // - this region exclusively owns its covered range (required by safety constraints)
        // - the immutable receiver excludes stores for this entire range
        Ok(unsafe { self.owner.borrow().load32_unchecked(offset) })
    }

    fn try_load64(&self, offset: usize) -> Result<u64, MmioError> {
        let offset = self.resolve_offset::<u64>(offset)?;
        // Safety:
        // - this region exclusively owns its covered range (required by safety constraints)
        // - the immutable receiver excludes stores for this entire range
        Ok(unsafe { self.owner.borrow().load64_unchecked(offset) })
    }

    fn try_store8(&mut self, offset: usize, v: u8) -> Result<(), MmioError> {
        let offset = self.resolve_offset::<u8>(offset)?;
        // Safety:
        // - this region exclusively owns its covered range (required by safety constraints)
        // - the mutable receiver excludes all other operations for this entire range
        unsafe {
            self.owner.borrow().store8_unchecked(offset, v);
        }
        Ok(())
    }

    fn try_store16(&mut self, offset: usize, v: u16) -> Result<(), MmioError> {
        let offset = self.resolve_offset::<u16>(offset)?;
        // Safety:
        // - this region exclusively owns its covered range (required by safety constraints)
        // - the mutable receiver excludes all other operations for this entire range
        unsafe {
            self.owner.borrow().store16_unchecked(offset, v);
        }
        Ok(())
    }

    fn try_store32(&mut self, offset: usize, v: u32) -> Result<(), MmioError> {
        let offset = self.resolve_offset::<u32>(offset)?;
        // Safety:
        // - this region exclusively owns its covered range (required by safety constraints)
        // - the mutable receiver excludes all other operations for this entire range
        unsafe {
            self.owner.borrow().store32_unchecked(offset, v);
        }
        Ok(())
    }

    fn try_store64(&mut self, offset: usize, v: u64) -> Result<(), MmioError> {
        let offset = self.resolve_offset::<u64>(offset)?;
        // Safety:
        // - this region exclusively owns its covered range (required by safety constraints)
        // - the mutable receiver excludes all other operations for this entire range
        unsafe {
            self.owner.borrow().store64_unchecked(offset, v);
        }
        Ok(())
    }

    fn write_barrier(&self) {
        self.owner.borrow().write_barrier();
    }
}

impl<Impl: UnsafeMmio, Owner: Borrow<Impl> + Clone> MmioSplit for MmioRegion<Impl, Owner> {
    fn try_split_off(&mut self, mid: usize) -> Result<Self, MmioError> {
        if mid > self.len() {
            return Err(MmioError::OutOfRange);
        }

        // Resolve the midpoint to an absolute offset.
        let mid = self.bounds.start + mid;

        // Split the bounds into two disjoint ranges.
        let lhs = self.bounds.start..mid;
        let rhs = mid..self.bounds.end;

        // Relinquish ownership of the lhs.
        self.bounds = rhs;

        // Safety:
        // - this region exclusively owns its covered range (required by safety constraints)
        // - the mutable receiver excludes all other operations for this entire range
        // - this mmio region splits off a portion of its owned range and relinquishes ownership of
        // it before returning
        // - the returned MmioRegion owns a range that was owned by this MmioRegion at the start of
        // this call and no longer is
        Ok(unsafe { Self::new_unchecked(self.owner.clone(), lhs) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MmioOperand;
    use fuchsia_sync::RwLock;
    use rand::RngExt as _;
    use std::sync::Barrier;
    use std::thread::sleep;
    use std::time::Duration;

    /// An UnsafeMmio implementation that dynamically checks violations of the safety requirements:
    /// - a store concurrent with another operation on a memory range
    /// - an unaligned access
    ///
    /// This implementation will panic if an unaligned operation is issued.
    ///
    /// This implementation *might* panic on unsafe concurrent usage. If it does panic in this case
    /// there was unsafe concurrent usage, however the lack of a panic doesn't guarantee all usage
    /// was safe. The mean_op_duration parameter to new controls how long the average borrow will
    /// last - increasing this can make it more likely that unsafe usage will be detected.
    struct CheckedRegisters {
        cells: Vec<RwLock<u8>>,
        mean_op_duration: f32,
    }

    impl CheckedRegisters {
        fn new(len: usize, mean_op_duration: Duration) -> Self {
            let mut cells = Vec::new();
            cells.resize_with(len, || RwLock::new(0));

            let mean_op_duration = mean_op_duration.as_secs_f32();

            Self { cells, mean_op_duration }
        }

        fn sleep(&self) {
            // model op duration as a poisson process to get some jitter.
            let uniform_sample: f32 = rand::random::<f32>().max(0.000001);
            let duration_secs = -self.mean_op_duration * uniform_sample.ln();
            sleep(Duration::from_secs_f32(duration_secs));
        }

        fn load<const N: usize>(&self, start: usize) -> [u8; N] {
            let borrows: [_; N] = core::array::from_fn(|i| {
                self.cells[start + i]
                    .try_read()
                    .expect("attempt to load from an address that is being stored to")
            });

            // Sleep while borrowing these cells to increase the chance that unsafe usage will be
            // detected.
            self.sleep();

            borrows.map(|r| *r)
        }

        fn store<const N: usize>(&self, start: usize, bytes: [u8; N]) {
            let borrows: [_; N] = core::array::from_fn(|i| {
                self.cells[start + i]
                    .try_write()
                    .expect("attempt to store to an address concurrently with another operation")
            });

            // Sleep while borrowing these cells to increase the chance that unsafe usage will be
            // detected.
            self.sleep();

            borrows.into_iter().zip(bytes).for_each(|(mut r, b)| *r = b);
        }
    }

    impl UnsafeMmio for CheckedRegisters {
        fn len(&self) -> usize {
            self.cells.len()
        }

        fn align_offset(&self, _align: usize) -> usize {
            0
        }

        unsafe fn load8_unchecked(&self, offset: usize) -> u8 {
            self.load::<1>(offset)[0]
        }

        unsafe fn load16_unchecked(&self, offset: usize) -> u16 {
            assert_eq!(offset % 2, 0);
            u16::from_le_bytes(self.load::<2>(offset))
        }

        unsafe fn load32_unchecked(&self, offset: usize) -> u32 {
            assert_eq!(offset % 4, 0);
            u32::from_le_bytes(self.load::<4>(offset))
        }

        unsafe fn load64_unchecked(&self, offset: usize) -> u64 {
            assert_eq!(offset % 8, 0);
            u64::from_le_bytes(self.load::<8>(offset))
        }

        unsafe fn store8_unchecked(&self, offset: usize, v: u8) {
            self.store::<1>(offset, [v]);
        }

        unsafe fn store16_unchecked(&self, offset: usize, v: u16) {
            assert_eq!(offset % 2, 0);
            self.store::<2>(offset, v.to_le_bytes())
        }

        unsafe fn store32_unchecked(&self, offset: usize, v: u32) {
            assert_eq!(offset % 4, 0);
            self.store::<4>(offset, v.to_le_bytes())
        }

        unsafe fn store64_unchecked(&self, offset: usize, v: u64) {
            assert_eq!(offset % 8, 0);
            self.store::<8>(offset, v.to_le_bytes())
        }

        fn write_barrier(&self) {
            // NOP
        }
    }

    #[test]
    fn test_memory_region_thread_safety() {
        // The number of concurrent threads.
        const CONCURRENCY: usize = 64;

        // The number of bytes each thread owns. Must be a non-zero multiple of 8.
        const BYTES_PER_THREAD: usize = 8;

        // The average time for an operation to hold a borrow.
        const MEAN_OP_TIME: Duration = Duration::from_micros(100);

        // The number of ops to perform per thread. At 100us per op the minimum sleep time per
        // thread should be around 0.5s.
        const THREAD_OP_COUNT: usize = 5000;

        // The total size of the Mmio region.
        const LEN: usize = CONCURRENCY * BYTES_PER_THREAD;

        // These are required for test correctness.
        assert_ne!(BYTES_PER_THREAD, 0);
        assert_eq!(BYTES_PER_THREAD % 8, 0);

        let registers = CheckedRegisters::new(LEN, MEAN_OP_TIME);
        // Safety:
        // - CheckedRegisters only references memory it owns
        // - MmioRegion takes ownership of the CheckedRegisters object
        let mut region = MmioRegion::new(registers).into_split_send();

        let barrier = Barrier::new(CONCURRENCY);

        std::thread::scope(|s| {
            let barrier = &barrier;
            for _ in 0..CONCURRENCY {
                let mut split = region.split_off(BYTES_PER_THREAD);
                s.spawn(move || {
                    let mut rng = rand::rng();

                    // Wait until threads are ready to start to increase the chance of a race.
                    barrier.wait();

                    for _i in 0..THREAD_OP_COUNT {
                        let offset = rng.random_range(0..BYTES_PER_THREAD);
                        let op = rng.random_range(0usize..8);

                        let size = 1 << (op % 4);
                        // Choose a random offset from 0 to 2x the size of this region. MmioRegion
                        // should prevent reading out of bounds.
                        let offset = offset.next_multiple_of(size) % (BYTES_PER_THREAD * 2);

                        // We don't care whether these operations fail.
                        let _ = match op {
                            0 => split.try_load8(offset).err(),
                            1 => split.try_load16(offset).err(),
                            2 => split.try_load32(offset).err(),
                            3 => split.try_load64(offset).err(),
                            4 => split.try_store8(offset, rng.random()).err(),
                            5 => split.try_store16(offset, rng.random()).err(),
                            6 => split.try_store32(offset, rng.random()).err(),
                            7 => split.try_store64(offset, rng.random()).err(),
                            _ => unreachable!(),
                        };
                    }
                });
            }
        });
    }

    #[test]
    fn test_alignment() {
        const LEN: usize = 64;
        let registers = CheckedRegisters::new(LEN, Duration::ZERO);
        let mut region = MmioRegion::new(registers).into_split();
        let mut rng = rand::rng();

        fn assert_alignment<M: Mmio, T: MmioOperand>(
            mmio: &mut M,
            offset: usize,
            region_offset: usize,
            v: T,
        ) {
            let absolute_offset = offset + region_offset;
            let is_aligned = absolute_offset.is_multiple_of(align_of::<T>());
            let expected_res = if is_aligned { Ok(()) } else { Err(MmioError::Unaligned) };
            assert_eq!(mmio.check_suitable_for::<T>(offset), expected_res);
            assert_eq!(mmio.try_store(offset, v), expected_res);
            assert_eq!(mmio.try_load(offset), expected_res.map(|_| v));
        }

        for region_offset in 0..8 {
            // Do at least two cycles of the largest operand alignment to test modular arithmetic.
            for relative_offset in 0..16 {
                let v: u64 = rng.random();
                assert_alignment(&mut region, relative_offset, region_offset, v as u8);
                assert_alignment(&mut region, relative_offset, region_offset, v as u16);
                assert_alignment(&mut region, relative_offset, region_offset, v as u32);
                assert_alignment(&mut region, relative_offset, region_offset, v);
            }
            // Throw away the first byte to advance the region's bounds.
            let _ = region.split_off(1);
        }
    }

    #[test]
    fn test_wrapped_alignment() {
        // Test all combinations for how the wrapped MMIO regions alignment, region start and region
        // relative offsets may stack with respect to alignment.
        struct OffsetMmio(usize);
        impl UnsafeMmio for OffsetMmio {
            fn len(&self) -> usize {
                isize::MAX as usize
            }

            fn align_offset(&self, align: usize) -> usize {
                align.wrapping_sub(self.0) % align
            }

            unsafe fn load8_unchecked(&self, _offset: usize) -> u8 {
                unreachable!()
            }

            unsafe fn load16_unchecked(&self, _offset: usize) -> u16 {
                unreachable!()
            }

            unsafe fn load32_unchecked(&self, _offset: usize) -> u32 {
                unreachable!()
            }

            unsafe fn load64_unchecked(&self, _offset: usize) -> u64 {
                unreachable!()
            }

            unsafe fn store8_unchecked(&self, _offset: usize, _value: u8) {
                unreachable!()
            }

            unsafe fn store16_unchecked(&self, _offset: usize, _value: u16) {
                unreachable!()
            }

            unsafe fn store32_unchecked(&self, _offset: usize, _value: u32) {
                unreachable!()
            }

            unsafe fn store64_unchecked(&self, _offset: usize, _value: u64) {
                unreachable!()
            }

            fn write_barrier(&self) {
                unreachable!()
            }
        }

        // Loop through all combinations of the wrapped offset, region_start and relative_offset
        // for 2x the size of the largest operand in order to test all combinations of these in the
        // face of the modular arithmetic.
        for wrapped_offset in 0..16 {
            let offset_mmio = OffsetMmio(wrapped_offset);
            let mut region = MmioRegion::new(offset_mmio).into_split();

            for region_start in 0..16 {
                let absolute_region_start = wrapped_offset + region_start;
                assert_eq!(region.align_offset(1), 0);
                assert_eq!(region.align_offset(2), 2_usize.wrapping_sub(absolute_region_start) % 2);
                assert_eq!(region.align_offset(4), 4_usize.wrapping_sub(absolute_region_start) % 4);
                assert_eq!(region.align_offset(8), 8_usize.wrapping_sub(absolute_region_start) % 8);

                for relative_offset in 0..16 {
                    let absolute_offset = wrapped_offset + region_start + relative_offset;

                    // Every offset is suitably aligned for u8.
                    assert_eq!(region.check_aligned_for::<u8>(relative_offset), Ok(()));
                    assert_eq!(
                        region.check_aligned_for::<u16>(relative_offset),
                        if absolute_offset % 2 == 0 { Ok(()) } else { Err(MmioError::Unaligned) }
                    );
                    assert_eq!(
                        region.check_aligned_for::<u32>(relative_offset),
                        if absolute_offset % 4 == 0 { Ok(()) } else { Err(MmioError::Unaligned) }
                    );
                    assert_eq!(
                        region.check_aligned_for::<u64>(relative_offset),
                        if absolute_offset % 8 == 0 { Ok(()) } else { Err(MmioError::Unaligned) }
                    );
                }
                // Drop the first byte to advance the region's offset.
                let _ = region.split_off(1);
            }
        }
    }
}
