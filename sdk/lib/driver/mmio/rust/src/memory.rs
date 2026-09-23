// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! MMIO regions backed by memory.
//!
//! This module defines the primary [Mmio] implementation, backed by memory.

use crate::arch;
use crate::region::{MmioRegion, UnsafeMmio};
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr::NonNull;

/// An exclusively owned region of memory.
pub struct Memory<Claim> {
    base_ptr: NonNull<u8>,
    len: usize,
    _claim: Claim,
}

// Safety: it is safe to send the pointer to another thread as accessing the pointer requires
// handling thread safety.
unsafe impl<Claim: Send> Send for Memory<Claim> {}

// Safety it is safe to access this pointer from multiple threads as accessing it already requires
// handling thread safety.
unsafe impl<Claim> Sync for Memory<Claim> {}

impl<Claim> Memory<Claim> {
    /// Creates an instance of UnsafeMemory representing the region starting at `base_ptr` and
    /// extending for the subsequent `len` bytes.
    ///
    /// # Safety
    /// - The given memory must be exclusively owned by the returned object for the lifetime of the
    ///   claim.
    pub unsafe fn new_unchecked(claim: Claim, base_ptr: NonNull<u8>, len: usize) -> Self {
        Self { base_ptr, len, _claim: claim }
    }

    /// Retrieves a valid pointer to `T` at `offset`.
    ///
    /// # Panics
    ///
    /// Panics if `T` is not properly aligned at `offset` or if `Memory` would
    /// be out of range for the size of `T`.
    pub fn ptr<T>(&self, offset: usize) -> NonNull<T> {
        // If this fails the caller has not met the safety requirements. It's safer to panic than it
        // is to continue.
        assert!((offset + size_of::<T>()) <= self.len && offset <= isize::MAX as usize);
        let ptr = unsafe { self.base_ptr.add(offset) };
        let ptr = ptr.cast::<T>();
        // If this fails the caller has not met the safety requirements. It's safer to panic than
        // it is to continue.
        assert!(ptr.is_aligned());
        ptr
    }
}

impl<Claim> UnsafeMmio for Memory<Claim> {
    fn len(&self) -> usize {
        self.len
    }

    fn align_offset(&self, align: usize) -> usize {
        self.base_ptr.align_offset(align)
    }

    unsafe fn load8_unchecked(&self, offset: usize) -> u8 {
        let ptr = self.ptr::<u8>(offset);
        // Safety: provided by this function's safety requirements.
        unsafe { arch::load8(ptr) }
    }

    unsafe fn load16_unchecked(&self, offset: usize) -> u16 {
        let ptr = self.ptr::<u16>(offset);
        // Safety: provided by this function's safety requirements.
        unsafe { arch::load16(ptr) }
    }

    unsafe fn load32_unchecked(&self, offset: usize) -> u32 {
        let ptr = self.ptr::<u32>(offset);
        // Safety: provided by this function's safety requirements.
        unsafe { arch::load32(ptr) }
    }

    unsafe fn load64_unchecked(&self, offset: usize) -> u64 {
        let ptr = self.ptr::<u64>(offset);
        // Safety: provided by this function's safety requirements.
        unsafe { arch::load64(ptr) }
    }

    unsafe fn store8_unchecked(&self, offset: usize, v: u8) {
        let ptr = self.ptr::<u8>(offset);
        // Safety: provided by this function's safety requirements.
        unsafe {
            arch::store8(ptr, v);
        }
    }

    unsafe fn store16_unchecked(&self, offset: usize, v: u16) {
        let ptr = self.ptr::<u16>(offset);
        // Safety: provided by this function's safety requirements.
        unsafe {
            arch::store16(ptr, v);
        }
    }

    unsafe fn store32_unchecked(&self, offset: usize, v: u32) {
        let ptr = self.ptr::<u32>(offset);
        // Safety: provided by this function's safety requirements.
        unsafe {
            arch::store32(ptr, v);
        }
    }

    unsafe fn store64_unchecked(&self, offset: usize, v: u64) {
        let ptr = self.ptr::<u64>(offset);
        // Safety: provided by this function's safety requirements.
        unsafe {
            arch::store64(ptr, v);
        }
    }

    fn write_barrier(&self) {
        arch::write_barrier();
    }
}

/// Represents a mutable borrow for the lifetime `'a`.
///
/// This represents a claim valid for the lifetime `'a`, used when borrowing memory through mutable
/// references.
pub struct MutableBorrow<'a>(PhantomData<&'a mut u8>);

impl<'a> Memory<MutableBorrow<'a>> {
    /// Borrows an `MmioRegion` backed by the uninitialized memory.
    pub fn borrow_uninit<T>(mem: &'a mut MaybeUninit<T>) -> MmioRegion<Self> {
        let len = size_of_val(mem);
        let base_ptr = NonNull::from(mem).cast::<u8>();
        // Safety:
        // - the returned memory takes a mutable borrow on the passed mem.
        // - the returned memory range is completely within the passed mem.
        MmioRegion::new(unsafe { Self::new_unchecked(MutableBorrow(PhantomData), base_ptr, len) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mmio, MmioError};

    #[test]
    fn test_aligned_access() {
        const NUM_REGISTERS: usize = 32;
        let mut mem = MaybeUninit::<[u64; NUM_REGISTERS]>::zeroed();
        let size = size_of_val(&mem);

        {
            let mut mmio = Memory::borrow_uninit(&mut mem);
            assert_eq!(size, mmio.len());
            for i in 0..NUM_REGISTERS {
                // check this returns the initial register memory state.
                assert_eq!(mmio.load64(i * size_of::<u64>()), 0);
            }

            for i in 0..NUM_REGISTERS {
                // write into the register memory.
                mmio.store64(i * size_of::<u64>(), i as u64);
            }

            for i in 0..NUM_REGISTERS {
                // ensure the stores occurred.
                assert_eq!(mmio.load64(i * size_of::<u64>()), i as u64);
            }
        }

        // Safety: any value of [u64; N] us valid.
        let registers = unsafe { mem.assume_init() };

        for (i, v) in registers.iter().copied().enumerate() {
            // ensure the stores modified the underlying memory range.
            assert_eq!(i as u64, v);
        }
    }

    #[test]
    fn test_alignment_checking() {
        const NUM_REGISTERS: usize = 32;
        let mut mem = MaybeUninit::<[u64; NUM_REGISTERS]>::zeroed();
        let mut mmio = Memory::borrow_uninit(&mut mem);

        for i in 0..32 {
            assert_eq!(mmio.try_load8(i), Ok(0));
            assert_eq!(mmio.try_store8(i, 0), Ok(()));

            if i % 2 == 0 {
                assert_eq!(mmio.try_load16(i), Ok(0));
                assert_eq!(mmio.try_store16(i, 0), Ok(()));
            } else {
                assert_eq!(mmio.try_load16(i), Err(MmioError::Unaligned));
                assert_eq!(mmio.try_store16(i, 0), Err(MmioError::Unaligned));
            }

            if i % 4 == 0 {
                assert_eq!(mmio.try_load32(i), Ok(0));
                assert_eq!(mmio.try_store32(i, 0), Ok(()));
            } else {
                assert_eq!(mmio.try_load32(i), Err(MmioError::Unaligned));
                assert_eq!(mmio.try_store32(i, 0), Err(MmioError::Unaligned));
            }

            if i % 8 == 0 {
                assert_eq!(mmio.try_load64(i), Ok(0));
                assert_eq!(mmio.try_store64(i, 0), Ok(()));
            } else {
                assert_eq!(mmio.try_load64(i), Err(MmioError::Unaligned));
                assert_eq!(mmio.try_store64(i, 0), Err(MmioError::Unaligned));
            }
        }
    }

    #[test]
    fn test_bounds_checking() {
        const NUM_REGISTERS: usize = 32;
        let mut mem = MaybeUninit::<[u64; NUM_REGISTERS]>::zeroed();
        let size = size_of_val(&mem);
        let mut mmio = Memory::borrow_uninit(&mut mem);

        assert_eq!(mmio.try_load8(size), Err(MmioError::OutOfRange));
        assert_eq!(mmio.try_store8(size, 0), Err(MmioError::OutOfRange));

        assert_eq!(mmio.try_load16(size), Err(MmioError::OutOfRange));
        assert_eq!(mmio.try_store16(size, 0), Err(MmioError::OutOfRange));

        assert_eq!(mmio.try_load32(size), Err(MmioError::OutOfRange));
        assert_eq!(mmio.try_store32(size, 0), Err(MmioError::OutOfRange));

        assert_eq!(mmio.try_load64(size), Err(MmioError::OutOfRange));
        assert_eq!(mmio.try_store64(size, 0), Err(MmioError::OutOfRange));
    }

    #[test]
    fn test_aliased_access() {
        const NUM_REGISTERS: usize = 32;
        let mut mem = MaybeUninit::<[u64; NUM_REGISTERS]>::zeroed();
        let mut mmio = Memory::borrow_uninit(&mut mem);

        fn test_bytes(i: usize) -> [u8; 8] {
            let i = i as u8;
            [
                i,
                i.wrapping_mul(3),
                i.wrapping_mul(5),
                i.wrapping_mul(7),
                i.wrapping_mul(11),
                i.wrapping_mul(13),
                i.wrapping_mul(17),
                i.wrapping_mul(19),
            ]
        }

        for i in 0..NUM_REGISTERS {
            let v = u64::from_le_bytes(test_bytes(i));
            mmio.store64(i * size_of::<u64>(), v.to_le());
        }

        for i in 0..NUM_REGISTERS / 4 {
            let bytes = test_bytes(i);
            let v64 = u64::from_le_bytes(bytes).to_le();
            let v32s = [
                u32::from_le_bytes(bytes[..4].try_into().unwrap()).to_le(),
                u32::from_le_bytes(bytes[4..].try_into().unwrap()).to_le(),
            ];
            let v16s = [
                u16::from_le_bytes(bytes[..2].try_into().unwrap()).to_le(),
                u16::from_le_bytes(bytes[2..4].try_into().unwrap()).to_le(),
                u16::from_le_bytes(bytes[4..6].try_into().unwrap()).to_le(),
                u16::from_le_bytes(bytes[6..8].try_into().unwrap()).to_le(),
            ];
            let v8s = bytes;

            let base_offset = 4 * i * size_of::<u64>();

            let r0 = base_offset;
            let r1 = base_offset + size_of::<u64>();
            let r2 = base_offset + 2 * size_of::<u64>();
            let r3 = base_offset + 3 * size_of::<u64>();

            // Store each register with a different granularity.
            mmio.store64(r0, v64);
            for (j, v) in v32s.iter().enumerate() {
                mmio.store32(r1 + j * size_of::<u32>(), *v);
            }
            for (j, v) in v16s.iter().enumerate() {
                mmio.store16(r2 + j * size_of::<u16>(), *v);
            }
            for (j, v) in v8s.iter().enumerate() {
                mmio.store8(r3 + j * size_of::<u8>(), *v);
            }

            // Now test loading each register back at the different granularities.
            for j in 0..4 {
                let r = base_offset + j * size_of::<u64>();
                assert_eq!(mmio.load64(r), v64);

                for (j, v) in v32s.iter().enumerate() {
                    assert_eq!(mmio.load32(r + j * size_of::<u32>()), *v);
                }

                for (j, v) in v16s.iter().enumerate() {
                    assert_eq!(mmio.load16(r + j * size_of::<u16>()), *v);
                }

                for (j, v) in v8s.iter().enumerate() {
                    assert_eq!(mmio.load8(r + j), *v);
                }
            }
        }
    }

    #[test]
    fn test_masked_access() {
        use crate::mmio::MmioExt;

        let mut mem = MaybeUninit::<u64>::zeroed();
        let mut mmio = Memory::borrow_uninit(&mut mem);

        mmio.store(0, 0xfedcba98_76543210_u64);
        assert_eq!(0xfedcba98_76543210, mmio.masked_load(0, u64::MAX));
        assert_eq!(0x00000000_76543210, mmio.masked_load(0, u32::MAX as u64));
        assert_eq!(0xfedcba98_00000000, mmio.masked_load(0, (u32::MAX as u64) << 32));
        assert_eq!(
            0xfedcba98_76543210 & 0xaaaaaaaa_aaaaaaaa,
            mmio.masked_load(0, 0xaaaaaaaa_aaaaaaaa_u64)
        );

        mmio.masked_modify(0, 0xff000000_00000000u64, 0);
        assert_eq!(0x00dcba98_76543210, mmio.masked_load(0, u64::MAX));
    }
}
