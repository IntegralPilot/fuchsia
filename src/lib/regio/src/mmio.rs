// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;

use super::{
    AccessRestrictsTo, Accessible, IoHandle, LayoutOver, NoWrite, ReadHandle, Readable, Register,
    Writable, WriteHandle,
};

/// Represents an in-memory register with a given layout and access permissions
/// located at an offset from a base address.
#[derive(Debug)]
pub struct Offset<Layout, Access> {
    /// The offset in bytes.
    pub value: usize,
    _marker: PhantomData<(Layout, Access)>,
}

impl<Layout, Access> Offset<Layout, Access> {
    pub const fn new(value: usize) -> Self {
        Self { value, _marker: PhantomData }
    }
}

//
// We manually implement Clone and Copy, without having to make further
// assumptions about `Access`.
//

impl<Layout, Access> Clone for Offset<Layout, Access> {
    fn clone(&self) -> Self {
        Self { value: self.value, _marker: PhantomData }
    }
}

impl<Layout, Access> Copy for Offset<Layout, Access> {}

/// A specialization of [`Register`] representing an MMIO address.
pub type Mmio<Layout, Base, Access> = Register<Layout, Access, MmioPtr<Base, Access>>;

impl<Layout, Base, Access> Mmio<Layout, Base, Access>
where
    Layout: LayoutOver<Base>,
    Base: Copy,
    Access: Accessible,
{
    /// Constructs a new memory-backed register from a pointer wrapper.
    pub const fn new<InputAccess>(ptr: MmioPtr<Base, InputAccess>) -> Self
    where
        InputAccess: AccessRestrictsTo<Access>,
    {
        // Safety: MmioPtr's construction attested to both the handle-specific
        // access preconditions and that the access permissions are correctly
        // modeled; restriction only narrows the latter claim.
        unsafe { Register::from_io(ptr.restrict()) }
    }
}

/// Represents a pointer to an MMIO address - with specific access
/// permissions - at which a register might be located.
#[derive(Debug)]
pub struct MmioPtr<T, Access: Accessible>(*const T, PhantomData<Access>);

//
// Read-only pointers will deal in *const T, while writable ones will deal in
// *mut T.
//

impl<T, R> MmioPtr<T, (R, NoWrite)>
where
    (R, NoWrite): Accessible,
{
    /// Constructs a new, read-only MMIO pointer wrapper.
    ///
    /// # Safety
    ///
    /// The caller must guarantee the safety preconditions of
    /// [`core::ptr::read_volatile()`] for the lifetime of [`MmioPtr`]. In
    /// particular...
    ///
    /// * that `ptr` is aligned and points to a mapped MMIO address;
    ///
    /// * the mapping remains valid for the lifetime of the returned `MmioPtr`
    ///   (and any copies of it);
    ///
    /// * accesses must never trap.
    ///
    /// Note that it is not assumed that the backing memory is immutable.
    pub const unsafe fn new(ptr: *const T) -> Self {
        Self(ptr, PhantomData)
    }

    /// Returns the underlying pointer.
    pub const fn as_ptr(self) -> *const T {
        self.0
    }
}

impl<T, Access: Writable> MmioPtr<T, Access> {
    /// Constructs a new, writable MMIO pointer wrapper.
    ///
    /// # Safety
    ///
    /// The caller must guarantee the safety preconditions of
    /// [`core::ptr::read_volatile()`] (if readable) and
    /// [`core::ptr::write_volatile()`] for the lifetime of [`MmioPtr`]. In
    /// particular...
    ///
    /// * that `ptr` is aligned and points to a mapped MMIO address;
    ///
    /// * the mapping remains valid for the lifetime of the returned `MmioPtr`
    ///   (and any copies of it);
    ///
    /// * accesses must never trap.
    ///
    /// Also,
    ///
    /// * If `Access` expresses safe-writability (see
    /// [`SafeWrite`](crate::SafeWrite)), that writing any value whatsoever to
    /// this address cannot result in undefined behaviour.
    ///
    /// Note that it is not assumed that the backing memory is immutable, or
    /// that a writable [`MmioPtr`] has exclusive access to it.
    pub const unsafe fn new(ptr: *mut T) -> Self {
        Self(ptr.cast(), PhantomData)
    }

    /// Returns the underlying pointer.
    pub const fn as_ptr(self) -> *mut T {
        // `MmioPtr`s with writable access were necessarily constructed with a
        // mutable pointer (see above).
        self.0.cast_mut()
    }
}

impl<T, Access: Accessible> MmioPtr<T, Access> {
    /// Restricts the access permissions of this pointer.
    pub const fn restrict<NewAccess>(self) -> MmioPtr<T, NewAccess>
    where
        NewAccess: Accessible,
        Access: AccessRestrictsTo<NewAccess>,
    {
        MmioPtr(self.0, PhantomData)
    }
}

//
// We manually implement Clone and Copy (as expected of a pointer), without
// having to make further assumptions about `Access`.
//

impl<T, Access: Accessible> Clone for MmioPtr<T, Access> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<T, Access: Accessible> Copy for MmioPtr<T, Access> {}

impl<T, Access> IoHandle for MmioPtr<T, Access>
where
    T: Copy,
    Access: Accessible,
{
    type Base = T;
}

macro_rules! impl_mmio_ptr_handle {
    ($ty:ty, $read_fn:ident, $write_fn:ident) => {
        impl<Access> ReadHandle for MmioPtr<$ty, Access>
        where
            Access: Readable,
        {
            #[inline(always)]
            unsafe fn read_raw(&self) -> $ty {
                unsafe { mmio_ptr::$read_fn(self.0) }
            }
        }

        impl<Access> WriteHandle for MmioPtr<$ty, Access>
        where
            Access: Writable,
        {
            #[inline(always)]
            unsafe fn write_raw(&self, value: $ty) {
                // Regarding the `cast_mut()`, `MmioPtr`s with writable access were
                // necessarily constructed with a mutable pointer (see above).
                unsafe { mmio_ptr::$write_fn(value, self.0.cast_mut()) }
            }
        }
    };
}

impl_mmio_ptr_handle!(u8, read8, write8);
impl_mmio_ptr_handle!(u16, read16, write16);
impl_mmio_ptr_handle!(u32, read32, write32);

#[cfg(target_pointer_width = "64")]
impl_mmio_ptr_handle!(u64, read64, write64);

// Safety: An MMIO address has no thread affinity.
unsafe impl<T, Access: Accessible> Send for MmioPtr<T, Access> {}

// Safety: All accesses through `MmioPtr` are volatile operations.
unsafe impl<T, Access: Accessible> Sync for MmioPtr<T, Access> {}

/// Represents a contiguous region of memory containing registers. Contained
/// registers are accessed via [`MmioBank::at`] and must feature access
/// permissions narrower than `MaxAccess`.
pub struct MmioBank<Base, MaxAccess: Accessible> {
    base: MmioPtr<Base, MaxAccess>,
    size_bytes: usize,
}

impl<Base, MaxAccess> MmioBank<Base, MaxAccess>
where
    Base: Copy,
    MaxAccess: Accessible,
{
    /// Constructs a new bank from its base pointer and length.
    pub const fn new(base: MmioPtr<Base, MaxAccess>, size_bytes: usize) -> Self {
        Self { base, size_bytes }
    }

    /// Returns the memory-backed register at the specified offset into the
    /// bank.
    ///
    /// # Panics
    ///
    /// Panics if the offset is misaligned for `Base` or exceeds the bounds of
    /// the bank.
    ///
    /// # Safety
    ///
    /// Same as [`MmioPtr::new`] for underlying offset pointer.
    pub const unsafe fn at<Layout, Access>(
        &self,
        offset: Offset<Layout, Access>,
    ) -> Mmio<Layout, Base, Access>
    where
        Layout: LayoutOver<Base>,
        Access: Accessible,
        MaxAccess: AccessRestrictsTo<Access>,
    {
        assert!(offset.value.is_multiple_of(align_of::<Base>()));
        assert!(size_of::<Base>() <= self.size_bytes);
        assert!(offset.value <= self.size_bytes - size_of::<Base>());

        // Safety: The offset pointer inherits the attested access
        // preconditions of `self.base` (including having been constructed from
        // a mutable pointer, if writable), with the asserts above ensuring
        // that it remains within the bank's bounds.
        let ptr = unsafe {
            let offset_addr = self.base.0.byte_add(offset.value);
            MmioPtr::<Base, MaxAccess>(offset_addr, PhantomData)
        };
        Mmio::new(ptr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Ro, RwSafe, RwUnsafe, WoSafe, WoUnsafe};

    #[test]
    fn mmio_ptr_ro() {
        let data = [10u32, 20u32];

        // Safety: obviously a valid pointer.
        let ptr = unsafe { MmioPtr::<u32, Ro>::new(data.as_ptr()) };

        // Compile-time test of the raw pointer constructor.
        //
        // Safety: obviously points to valid memory.
        let bank = MmioBank::new(ptr, size_of_val(&data));

        const OFFSET0: Offset<u32, Ro> = Offset::new(0);
        let reg0 = unsafe { bank.at(OFFSET0) }; // Safety: within bounds
        assert_eq!(reg0.read(), 10);

        const OFFSET4: Offset<u32, Ro> = Offset::new(4);
        let reg4 = unsafe { bank.at(OFFSET4) }; // Safety: within bounds
        assert_eq!(reg4.read(), 20);
    }

    #[test]
    fn mmio_ptr_wo_safe() {
        let mut data = [10u32, 20u32];

        // Safety: obviously a valid pointer.
        let ptr = unsafe { MmioPtr::<u32, WoSafe>::new(data.as_mut_ptr()) };
        let bank = MmioBank::new(ptr, size_of_val(&data));

        const OFFSET0: Offset<u32, WoSafe> = Offset::new(0);
        let reg0 = unsafe { bank.at(OFFSET0) }; // Safety: within bounds
        assert_eq!(data[0], 10);
        reg0.write(100u32);
        assert_eq!(data[0], 100);

        const OFFSET4: Offset<u32, WoSafe> = Offset::new(4);
        let reg4 = unsafe { bank.at(OFFSET4) }; // Safety: within bounds
        assert_eq!(data[1], 20);
        reg4.write(200u32);
        assert_eq!(data[1], 200);
    }

    #[test]
    fn mmio_ptr_wo_unsafe() {
        let mut data = [10u32, 20u32];

        // Safety: obviously a valid pointer.
        let ptr = unsafe { MmioPtr::<u32, WoUnsafe>::new(data.as_mut_ptr()) };
        let bank = MmioBank::new(ptr, size_of_val(&data));

        const OFFSET0: Offset<u32, WoUnsafe> = Offset::new(0);
        let reg0 = unsafe { bank.at(OFFSET0) }; // Safety: within bounds
        assert_eq!(data[0], 10);
        unsafe { reg0.write(100u32) }; // Unsafe required: WAI!
        assert_eq!(data[0], 100);

        const OFFSET4: Offset<u32, WoUnsafe> = Offset::new(4);
        let reg4 = unsafe { bank.at(OFFSET4) }; // Safety: within bounds
        assert_eq!(data[1], 20);
        unsafe { reg4.write(200u32) }; // Unsafe required: WAI!
        assert_eq!(data[1], 200);
    }

    #[test]
    fn mmio_ptr_rw_safe() {
        let mut data = [10u32, 20u32];

        // Safety: obviously a valid pointer.
        let ptr = unsafe { MmioPtr::<u32, RwSafe>::new(data.as_mut_ptr()) };
        let bank = MmioBank::new(ptr, size_of_val(&data));

        const OFFSET0: Offset<u32, RwSafe> = Offset::new(0);
        let reg0 = unsafe { bank.at(OFFSET0) }; // Within bounds
        assert_eq!(reg0.read(), 10);
        reg0.write(100u32);
        assert_eq!(reg0.read(), 100);
        assert_eq!(data[0], 100);

        const OFFSET4: Offset<u32, RwSafe> = Offset::new(4);
        let reg4 = unsafe { bank.at(OFFSET4) }; // Within bounds
        assert_eq!(reg4.read(), 20);
        reg4.write(200u32);
        assert_eq!(reg4.read(), 200);
        assert_eq!(data[1], 200);

        reg4.modify(|val| {
            assert_eq!(*val, 200);
            *val = 300;
        });
        assert_eq!(reg4.read(), 300);
        assert_eq!(data[1], 300);
    }

    #[test]
    fn mmio_ptr_rw_unsafe() {
        let mut data = [10u32, 20u32];

        // Safety: obviously a valid pointer.
        let ptr = unsafe { MmioPtr::<u32, RwUnsafe>::new(data.as_mut_ptr()) };
        let bank = MmioBank::new(ptr, size_of_val(&data));

        const OFFSET0: Offset<u32, RwUnsafe> = Offset::new(0);
        let reg0 = unsafe { bank.at(OFFSET0) }; // Within bounds
        assert_eq!(reg0.read(), 10);
        unsafe { reg0.write(100u32) }; // Unsafe required: WAI!
        assert_eq!(reg0.read(), 100);
        assert_eq!(data[0], 100);

        const OFFSET4: Offset<u32, RwUnsafe> = Offset::new(4);
        let reg4 = unsafe { bank.at(OFFSET4) }; // Within bounds
        assert_eq!(reg4.read(), 20);
        unsafe { reg4.write(200u32) }; // Unsafe required: WAI!
        assert_eq!(reg4.read(), 200);
        assert_eq!(data[1], 200);

        // Unsafe required: WAI!
        unsafe {
            reg4.modify(|val| {
                assert_eq!(*val, 200);
                *val = 300;
            });
        }
        assert_eq!(reg4.read(), 300);
        assert_eq!(data[1], 300);
    }

    #[test]
    fn mmio_bank_at_access_restrictions() {
        let mut data = [0u32; 1];
        let raw = data.as_mut_ptr();
        let size = size_of_val(&data);

        // Ro allows Ro
        {
            let base = unsafe { MmioPtr::<u32, Ro>::new(raw) };
            let bank = MmioBank::new(base, size);
            let _ = unsafe { bank.at(Offset::<u32, Ro>::new(0)) };
        }

        // WoSafe allows WoSafe, WoUnsafe
        {
            let base = unsafe { MmioPtr::<u32, WoSafe>::new(raw) };
            let bank = MmioBank::new(base, size);
            let _ = unsafe { bank.at(Offset::<u32, WoSafe>::new(0)) };
            let _ = unsafe { bank.at(Offset::<u32, WoUnsafe>::new(0)) };
        }

        // WoUnsafe allows WoUnsafe
        {
            let base = unsafe { MmioPtr::<u32, WoUnsafe>::new(raw) };
            let bank = MmioBank::new(base, size);
            let _ = unsafe { bank.at(Offset::<u32, WoUnsafe>::new(0)) };
        }

        // RwSafe allows RwSafe, RwUnsafe, WoSafe, WoUnsafe, Ro
        {
            let base = unsafe { MmioPtr::<u32, RwSafe>::new(raw) };
            let bank = MmioBank::new(base, size);
            let _ = unsafe { bank.at(Offset::<u32, RwSafe>::new(0)) };
            let _ = unsafe { bank.at(Offset::<u32, RwUnsafe>::new(0)) };
            let _ = unsafe { bank.at(Offset::<u32, WoSafe>::new(0)) };
            let _ = unsafe { bank.at(Offset::<u32, WoUnsafe>::new(0)) };
            let _ = unsafe { bank.at(Offset::<u32, Ro>::new(0)) };
        }

        // RwUnsafe allows RwUnsafe, WoUnsafe, Ro
        {
            let base = unsafe { MmioPtr::<u32, RwUnsafe>::new(raw) };
            let bank = MmioBank::new(base, size);
            let _ = unsafe { bank.at(Offset::<u32, RwUnsafe>::new(0)) };
            let _ = unsafe { bank.at(Offset::<u32, WoUnsafe>::new(0)) };
            let _ = unsafe { bank.at(Offset::<u32, Ro>::new(0)) };
        }
    }
}
