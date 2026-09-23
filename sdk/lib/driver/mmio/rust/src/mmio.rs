// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::register::{
    IndexedRegister, IndexedRegisterProxy, IndexedRegisterProxyMut, ReadableIndexedRegister,
    ReadableRegister, Register, RegisterProxy, RegisterProxyMut, WritableIndexedRegister,
    WritableRegister,
};
use core::fmt::{Debug, Display};
use core::ops::{BitAnd, BitOr, Not};

/// An error which may be encountered when performing operations on an MMIO region.
#[derive(Copy, Clone, PartialEq, Eq, Debug, thiserror::Error)]
pub enum MmioError {
    /// An operation was attempted outside the bounds of an MMIO region.
    #[error("An MMIO operation was out of range")]
    OutOfRange,
    /// An operation would be unaligned for the operand type at the given offset into the MMIO
    /// region.
    #[error("An MMIO operation was unaligned")]
    Unaligned,
}

/// An `Mmio` implementation provides access to an MMIO region.
///
/// # Device memory
/// For implementations that provide access to device memory, all memory must be accessed with
/// volatile semantics. I/O instructions should not be coalesced or cached and should occur in
/// program order.
///
/// Implementations are not required to provide any ordering guarantees between device memory
/// access and other instructions. Furthermore there is no guarantee that any side-effects are
/// visible to subsequent operations.
///
/// Callers who need guarantees around instruction ordering and side-effect visibility should
/// insert the appropriate compiler fences and/or memory barriers.
///
/// # Safety
/// Implementations of `Mmio` are required to be safe in a Rust sense, however loads may have
/// side-effects and device memory may change outside of the control of the host.
///
/// # Offsets, Alignment and Capacity
/// Mmio implementations provide access to device memory via offsets, not addresses. There are no
/// requirements on the alignment of offset `0`.
///
/// When using the load and store operations, callers are required to provide offsets representing
/// locations that are suitably aligned and fully within bounds of the location. Failure to do so
/// does not cause any safety issues, but may result in errors for the checked loads and stores, or
/// panics for the non-checked variants.
///
/// If these guarantees can't be provided statically for a given use case, the following functions
/// can be used to ensure these requirements:
///
/// - [MmioExt::check_aligned_for<T>]: returns an [MmioError::Unaligned] error if the given offset is
///   not suitably aligned.
/// - [MmioExt::check_capacity_for<T>]: returns an [MmioError::OutOfRange] error if there is not
///   sufficient capacity at the given offset.
/// - [MmioExt::check_suitable_for<T>]: returns an [MmioError] if there is not sufficient capacity at
///   the given offset or it is not suitably aligned.
///
/// # Dyn Compatibility
/// This trait is dyn compatible. See the [MmioExt] trait for useful utilities that extend this
/// trait.
pub trait Mmio {
    /// Returns the size in bytes of this MMIO region.
    fn len(&self) -> usize;

    /// Returns true if the MMIO region has a length of 0.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Computes the first offset within this region that is aligned to `align`.
    ///
    /// See the trait-level documentation for more information on offsets and alignment.
    ///
    /// # Panics
    /// If `align` is not a power of 2.
    fn align_offset(&self, align: usize) -> usize;

    /// Loads one byte from this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the load would exceed the bounds of this MMIO region.
    fn try_load8(&self, offset: usize) -> Result<u8, MmioError>;

    /// Loads one byte from this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Panics
    /// - If the load would exceed the bounds of this MMIO region.
    ///
    /// See [Mmio::try_load8] for a non-panicking version.
    fn load8(&self, offset: usize) -> u8 {
        self.try_load8(offset).unwrap()
    }

    /// Loads two bytes from this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the load would exceed the bounds of this MMIO region.
    /// - [MmioError::Unaligned]: if the offset is not suitably aligned.
    fn try_load16(&self, offset: usize) -> Result<u16, MmioError>;

    /// Loads two bytes from this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Panics
    /// - If the load would exceed the bounds of this MMIO region.
    /// - If the offset is not suitably aligned.
    ///
    /// See [Mmio::try_load16] for a non-panicking version.
    fn load16(&self, offset: usize) -> u16 {
        self.try_load16(offset).unwrap()
    }

    /// Loads four bytes from this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the load would exceed the bounds of this MMIO region.
    /// - [MmioError::Unaligned]: if the offset is not suitably aligned.
    fn try_load32(&self, offset: usize) -> Result<u32, MmioError>;

    /// Loads four bytes from this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Panics
    /// - If the load would exceed the bounds of this MMIO region.
    /// - If the offset is not suitably aligned.
    ///
    /// See [Mmio::try_load32] for a non-panicking version.
    fn load32(&self, offset: usize) -> u32 {
        self.try_load32(offset).unwrap()
    }

    /// Loads eight bytes from this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the load would exceed the bounds of this MMIO region.
    /// - [MmioError::Unaligned]: if the offset is not suitably aligned.
    fn try_load64(&self, offset: usize) -> Result<u64, MmioError>;

    /// Loads eight bytes from this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Panics
    /// - If the load would exceed the bounds of this MMIO region.
    /// - If the offset is not suitably aligned.
    ///
    /// See [Mmio::try_load64] for a non-panicking version.
    fn load64(&self, offset: usize) -> u64 {
        self.try_load64(offset).unwrap()
    }

    /// Stores one byte to this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the store would exceed the bounds of this MMIO region.
    fn try_store8(&mut self, offset: usize, value: u8) -> Result<(), MmioError>;

    /// Stores one byte to this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Panics
    /// - If the store would exceed the bounds of this MMIO region.
    ///
    /// See [Mmio::try_store8] for a non-panicking version.
    fn store8(&mut self, offset: usize, value: u8) {
        self.try_store8(offset, value).unwrap();
    }

    /// Stores two bytes to this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the store would exceed the bounds of this MMIO region.
    /// - [MmioError::Unaligned]: if the offset is not suitably aligned.
    fn try_store16(&mut self, offset: usize, value: u16) -> Result<(), MmioError>;

    /// Stores two bytes to this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Panics
    /// - If the store would exceed the bounds of this MMIO region.
    /// - If the offset is not suitably aligned.
    ///
    /// See [Mmio::try_store16] for a non-panicking version.
    fn store16(&mut self, offset: usize, value: u16) {
        self.try_store16(offset, value).unwrap();
    }

    /// Stores four bytes to this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the store would exceed the bounds of this MMIO region.
    /// - [MmioError::Unaligned]: if the offset is not suitably aligned.
    fn try_store32(&mut self, offset: usize, value: u32) -> Result<(), MmioError>;

    /// Stores four bytes to this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Panics
    /// - If the store would exceed the bounds of this MMIO region.
    /// - If the offset is not suitably aligned.
    ///
    /// See [Mmio::try_store32] for a non-panicking version.
    fn store32(&mut self, offset: usize, value: u32) {
        self.try_store32(offset, value).unwrap();
    }

    /// Stores eight bytes to this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the store would exceed the bounds of this MMIO region.
    /// - [MmioError::Unaligned]: if the offset is not suitably aligned.
    fn try_store64(&mut self, offset: usize, value: u64) -> Result<(), MmioError>;

    /// Stores eight bytes to this MMIO region at the given offset.
    ///
    /// See the trait-level documentation for information about offset requirements.
    ///
    /// # Panics
    /// - If the store would exceed the bounds of this MMIO region.
    /// - If the offset is not suitably aligned.
    ///
    /// See [Mmio::try_store64] for a non-panicking version.
    fn store64(&mut self, offset: usize, value: u64) {
        self.try_store64(offset, value).unwrap();
    }

    /// Issues a memory write barrier.  It is guaranteed that all stores preceding this barrier will
    /// appear to have happened before all stores following this barrier.
    fn write_barrier(&self);
}

/// An MMIO region from which ownership of disjoint sub-regions can be split off.
pub trait MmioSplit: Mmio + Sized {
    /// Splits this Mmio region into two at the given mid-point and returns the left-hand-side.
    /// This MMIO region's bounds are updated to contain only the right-hand-side.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if `mid > self.len()`.
    fn try_split_off(&mut self, mid: usize) -> Result<Self, MmioError>;

    /// Splits this MMIO region into two at the given mid-point and returns the left-hand-side.
    /// This MMIO region's bounds are updated to contain only the right-hand-side.
    ///
    /// # Panics
    /// - If `mid > self.len()`.
    fn split_off(&mut self, mid: usize) -> Self {
        self.try_split_off(mid).unwrap()
    }
}

/// Implemented for the fundamental types supported by all [Mmio] implementations.
///
/// This is a sealed trait, implemented for the types: u8, u16, u32, u64.
pub trait MmioOperand:
    BitAnd<Output = Self>
    + BitOr<Output = Self>
    + Not<Output = Self>
    + sealed::MmioOperand
    + PartialEq
    + Copy
    + Debug
    + Display
    + Sized
{
    /// Loads a value of the this type from the MMIO region at the given offset.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the load would exceed the bounds of this MMIO region.
    /// - [MmioError::Unaligned]: if the offset is not suitably aligned.
    fn try_load<M: Mmio + ?Sized>(mmio: &M, offset: usize) -> Result<Self, MmioError>;

    /// Stores a value of this type to the MMIO region at the given offset.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the store would exceed the bounds of this MMIO region.
    /// - [MmioError::Unaligned: if the offset is not suitably aligned.
    fn try_store<M: Mmio + ?Sized>(
        mmio: &mut M,
        offset: usize,
        value: Self,
    ) -> Result<(), MmioError>;
}

mod sealed {
    /// Internal sealed MmioOperand trait as the types implementing this must match the types in
    /// the loadn/storen trait methods.
    pub trait MmioOperand {}

    impl MmioOperand for u8 {}
    impl MmioOperand for u16 {}
    impl MmioOperand for u32 {}
    impl MmioOperand for u64 {}
}

impl MmioOperand for u8 {
    fn try_load<M: Mmio + ?Sized>(mmio: &M, offset: usize) -> Result<Self, MmioError> {
        mmio.try_load8(offset)
    }

    fn try_store<M: Mmio + ?Sized>(
        mmio: &mut M,
        offset: usize,
        value: Self,
    ) -> Result<(), MmioError> {
        mmio.try_store8(offset, value)
    }
}

impl MmioOperand for u16 {
    fn try_load<M: Mmio + ?Sized>(mmio: &M, offset: usize) -> Result<Self, MmioError> {
        mmio.try_load16(offset)
    }

    fn try_store<M: Mmio + ?Sized>(
        mmio: &mut M,
        offset: usize,
        value: Self,
    ) -> Result<(), MmioError> {
        mmio.try_store16(offset, value)
    }
}

impl MmioOperand for u32 {
    fn try_load<M: Mmio + ?Sized>(mmio: &M, offset: usize) -> Result<Self, MmioError> {
        mmio.try_load32(offset)
    }

    fn try_store<M: Mmio + ?Sized>(
        mmio: &mut M,
        offset: usize,
        value: Self,
    ) -> Result<(), MmioError> {
        mmio.try_store32(offset, value)
    }
}

impl MmioOperand for u64 {
    fn try_load<M: Mmio + ?Sized>(mmio: &M, offset: usize) -> Result<Self, MmioError> {
        mmio.try_load64(offset)
    }

    fn try_store<M: Mmio + ?Sized>(
        mmio: &mut M,
        offset: usize,
        value: Self,
    ) -> Result<(), MmioError> {
        mmio.try_store64(offset, value)
    }
}

/// This trait extends [Mmio] with some useful utilities. There is a blanket implementation for all
/// types implementing [Mmio].
///
/// Functions may go into this trait instead of [Mmio] if any of the following is true:
/// - their behavior shouldn't differ across Mmio implementations
/// - they would make Mmio not dyn compatible
/// - they would introduce an unnecessary burden on [Mmio] implementers
pub trait MmioExt: Mmio {
    /// Check that the given offset into this MMIO region would be suitable aligned for type `T`.
    /// There is no guarantee that this offset is within the bounds of the MMIO region or that
    /// there would be sufficient capacity to hold a `T` at that offset. See
    /// [MmioExt::check_suitable_for].
    ///
    /// Returns [MmioError::Unaligned] if the offset os not suitably aligned.
    fn check_aligned_for<T>(&self, offset: usize) -> Result<(), MmioError> {
        let align = align_of::<T>();
        let align_offset = self.align_offset(align);
        // An offset is aligned if (offset = align_offset + i * align) for some i.
        if offset.wrapping_sub(align_offset).is_multiple_of(align) {
            Ok(())
        } else {
            Err(MmioError::Unaligned)
        }
    }

    /// Checks that the given offset into this MMIO region has sufficient capacity to hold a value
    /// of type `T`. There is no guarantee that the offset is suitably aligned. See
    /// [MmioExt::check_capacity_for].
    fn check_capacity_for<T>(&self, offset: usize) -> Result<(), MmioError> {
        let capacity_at_offset = self.len().checked_sub(offset).ok_or(MmioError::OutOfRange)?;
        if capacity_at_offset >= size_of::<T>() { Ok(()) } else { Err(MmioError::OutOfRange) }
    }

    /// Checks that the given offset into this MMIO rgion is suitably aligned and has sufficient
    /// capacity for a value of type `T`.
    fn check_suitable_for<T>(&self, offset: usize) -> Result<(), MmioError> {
        self.check_aligned_for::<T>(offset)?;
        self.check_capacity_for::<T>(offset)?;
        Ok(())
    }

    /// Loads an [MmioOperand] from the MMIO region at the given offset.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the load would exceed the bounds of this MMIO region.
    /// - [MmioError::Unaligned]: if the offset is not suitably aligned.
    fn try_load<T: MmioOperand>(&self, offset: usize) -> Result<T, MmioError> {
        T::try_load(self, offset)
    }

    /// Loads an [MmioOperand] from the MMIO region at the given offset.
    ///
    /// # Panics
    /// If `self.check_suitable_for::<T>(offset)` would fail.
    ///
    /// See [MmioExt::try_load] for a non-panicking version.
    fn load<T: MmioOperand>(&self, offset: usize) -> T {
        self.try_load(offset).unwrap()
    }

    /// Stores an [MmioOperand] to the MMIO region at the given offset.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the store would exceed the bounds of this MMIO region.
    /// - [MmioError::Unaligned]: if the offset is not suitably aligned.
    fn try_store<T: MmioOperand>(&mut self, offset: usize, value: T) -> Result<(), MmioError> {
        T::try_store(self, offset, value)
    }

    /// Stores an [MmioOperand] to the MMIO region at the given offset.
    ///
    /// # Panics
    /// If `self.check_suitable_for::<T>(offset)` would fail.
    ///
    /// See [MmioExt::try_store] for a non-panicking version.
    fn store<T: MmioOperand>(&mut self, offset: usize, value: T) {
        self.try_store(offset, value).unwrap()
    }

    /// Loads an [MmioOperand] value from an MMIO region at the given offset, returning only the bits
    /// set in the given mask.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the load would exceed the bounds of this MMIO region.
    /// - [MmioError::Unaligned]: if the offset is not suitably aligned.
    fn try_masked_load<T: MmioOperand>(&self, offset: usize, mask: T) -> Result<T, MmioError> {
        self.try_load::<T>(offset).map(|v| v & mask)
    }

    /// Loads an [MmioOperand] value from an MMIO region at the given offset, returning only the bits
    /// set in the given mask.
    ///
    /// # Panics
    /// If `self.check_suitable_for::<T>(offset)` would fail.
    ///
    /// See [MmioExt::try_masked_load] for a non-panicking version.
    fn masked_load<T: MmioOperand>(&self, offset: usize, mask: T) -> T {
        self.try_masked_load::<T>(offset, mask).unwrap()
    }

    /// Updates the value in the MMIO region at the given offset, only modifying the bits set in
    /// the given mask.
    ///
    /// This operation performs a read-modify-write in order to only modify the bits matching the
    /// mask. As this is performed as a load from device memory followed by a store to device
    /// memory, the device may change state in between these operations.
    ///
    /// Callers must ensure that the this sequence of operations is valid for the device they're
    /// accessing and their use case.
    ///
    /// # Errors
    /// - [MmioError::OutOfRange]: if the load would exceed the bounds of this MMIO region.
    /// - [MmioError::Unaligned]: if the offset is not suitably aligned.
    fn try_masked_modify<T: MmioOperand>(
        &mut self,
        offset: usize,
        mask: T,
        value: T,
    ) -> Result<(), MmioError> {
        let current = self.try_load::<T>(offset)?;
        let unchanged_bits = current & !mask;
        let changed_bits = value & mask;
        self.try_store(offset, unchanged_bits | changed_bits)
    }

    /// Updates the value in the MMIO region at the given offset, only modifying the bits set in
    /// the given mask.
    ///
    /// This operation performs a read-modify-write in order to only modify the bits matching the
    /// mask. As this is performed as a load from device memory followed by a store to device
    /// memory, the device may change state in between these operations.
    ///
    /// Callers must ensure that the this sequence of operations is valid for the device they're
    /// accessing and their use case.
    ///
    /// # Panics
    ///
    /// If `self.check_suitable_for::<T>(offset)` would fail.
    ///
    /// See [MmioExt::try_masked_modify] for a non-panicking version.
    fn masked_modify<T: MmioOperand>(&mut self, offset: usize, mask: T, value: T) {
        self.try_masked_modify(offset, mask, value).unwrap()
    }

    /// Reads a [`ReadableRegister`] from the MMIO region.
    fn read_reg<R: ReadableRegister>(&self) -> R {
        R::read(self)
    }

    /// Writes a [`WritableRegister`] to the MMIO region.
    fn write_reg<R: WritableRegister>(&mut self, reg: R) {
        reg.write(self)
    }

    /// Reads a [`Register`] from the MMIO region, updates it with the given function, and writes it
    /// back.
    fn update_reg<R: ReadableRegister + WritableRegister, F>(&mut self, f: F)
    where
        F: FnOnce(&mut R),
    {
        let mut reg = self.read_reg::<R>();
        f(&mut reg);
        self.write_reg(reg);
    }

    /// Reads a [`ReadableIndexedRegister`] from the MMIO region at the given index.
    fn read_index_reg<R: ReadableIndexedRegister>(&self, index: usize) -> R {
        R::read_index(self, index)
    }

    /// Writes a [`WritableIndexedRegister`] to the MMIO region at the given index.
    fn write_index_reg<R: WritableIndexedRegister>(&mut self, index: usize, reg: R) {
        reg.write_index(self, index)
    }

    /// Reads an [`IndexedRegister`] from the MMIO region at the given index, updates it with the
    /// given function, and writes it back.
    fn update_index_reg<R: ReadableIndexedRegister + WritableIndexedRegister, F>(
        &mut self,
        index: usize,
        f: F,
    ) where
        F: FnOnce(&mut R),
    {
        let mut reg = self.read_index_reg::<R>(index);
        f(&mut reg);
        self.write_index_reg(index, reg);
    }

    /// Returns a [`RegisterProxy`] to ergonomically read a single `Register`.
    fn reg<R: Register>(&self) -> RegisterProxy<'_, Self, R> {
        RegisterProxy::new(self)
    }

    /// Returns a [`RegisterProxyMut`] to ergonomically mutate a single `Register`.
    fn reg_mut<R: Register>(&mut self) -> RegisterProxyMut<'_, Self, R> {
        RegisterProxyMut::new(self)
    }

    /// Returns an [`IndexedRegisterProxy`] to ergonomically read a single `IndexedRegister`.
    fn indexed_reg<R: IndexedRegister>(&self) -> IndexedRegisterProxy<'_, Self, R> {
        IndexedRegisterProxy::new(self)
    }

    /// Returns an [`IndexedRegisterProxyMut`] to ergonomically mutate a single `IndexedRegister`.
    fn indexed_reg_mut<R: IndexedRegister>(&mut self) -> IndexedRegisterProxyMut<'_, Self, R> {
        IndexedRegisterProxyMut::new(self)
    }
}

impl<M: Mmio + ?Sized> MmioExt for M {}
