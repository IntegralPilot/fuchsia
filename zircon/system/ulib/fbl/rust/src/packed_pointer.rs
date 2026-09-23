// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A provenance-safe packed pointer implementation.

/// A pointer wrapper that allows storing a small amount of data in the alignment bits of the
/// pointer.
///
/// The number of bits available for packing (`DATA_BITS`) must be less than or equal to the number
/// of trailing zero bits in the alignment of `T`.
///
/// `PackedPointer` enforces compile-time validation of the alignment constraints using const
/// assertions, and runtime debug assertions to verify pointer alignment and data limits.
///
/// If `CHECK_ALIGNMENT` is set to `false`, the compile-time alignment check is bypassed. This can
/// be useful when working with types whose alignment cannot be validated at compile-time.
///
/// This implementation preserves pointer provenance by using Rust's strict provenance methods
/// (`addr`, `with_addr`, `map_addr`).
#[repr(transparent)]
pub struct PackedPointer<T, const DATA_BITS: usize, const CHECK_ALIGNMENT: bool = true> {
    ptr: *mut T,
}

// Manually implement Clone and Copy since raw pointers are copyable.
impl<T, const DATA_BITS: usize, const CHECK_ALIGNMENT: bool> Clone
    for PackedPointer<T, DATA_BITS, CHECK_ALIGNMENT>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<T, const DATA_BITS: usize, const CHECK_ALIGNMENT: bool> Copy
    for PackedPointer<T, DATA_BITS, CHECK_ALIGNMENT>
{
}

// Manually implement PartialEq and Eq to compare the packed values.
impl<T, const DATA_BITS: usize, const CHECK_ALIGNMENT: bool> PartialEq
    for PackedPointer<T, DATA_BITS, CHECK_ALIGNMENT>
{
    fn eq(&self, other: &Self) -> bool {
        self.ptr == other.ptr
    }
}

impl<T, const DATA_BITS: usize, const CHECK_ALIGNMENT: bool> Eq
    for PackedPointer<T, DATA_BITS, CHECK_ALIGNMENT>
{
}

// Implement comparison with raw pointers.
impl<T, const DATA_BITS: usize, const CHECK_ALIGNMENT: bool> PartialEq<*mut T>
    for PackedPointer<T, DATA_BITS, CHECK_ALIGNMENT>
{
    fn eq(&self, other: &*mut T) -> bool {
        self.ptr() == *other
    }
}

impl<T, const DATA_BITS: usize, const CHECK_ALIGNMENT: bool> PartialEq<*const T>
    for PackedPointer<T, DATA_BITS, CHECK_ALIGNMENT>
{
    fn eq(&self, other: &*const T) -> bool {
        self.ptr() as *const T == *other
    }
}

impl<T, const DATA_BITS: usize, const CHECK_ALIGNMENT: bool>
    PackedPointer<T, DATA_BITS, CHECK_ALIGNMENT>
{
    const DATA_MASK: usize = (1 << DATA_BITS) - 1;
    const PTR_MASK: usize = !Self::DATA_MASK;

    const _ASSERT: () = {
        assert!(DATA_BITS > 0, "PackedPointer requires at least one data bit.");
        assert!(DATA_BITS < usize::BITS as usize, "Too many data bits requested.");
        if CHECK_ALIGNMENT {
            assert!(
                core::mem::align_of::<T>() >= (1 << DATA_BITS),
                "T has insufficient alignment for the requested number of data bits."
            );
        }
    };

    /// Creates a new `PackedPointer` from a pointer and data.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if the pointer is not aligned to the required boundary,
    /// or if the data exceeds the allowed number of bits.
    pub fn new(ptr: *mut T, data: usize) -> Self {
        let _ = Self::_ASSERT;
        debug_assert!(
            ptr.addr() & Self::DATA_MASK == 0,
            "Pointer {:?} is not aligned to at least {} bytes",
            ptr,
            1 << DATA_BITS
        );
        debug_assert!(data & Self::PTR_MASK == 0, "Data {} exceeds {} bits", data, DATA_BITS);

        let packed_addr = (ptr.addr() & Self::PTR_MASK) | (data & Self::DATA_MASK);
        Self { ptr: ptr.with_addr(packed_addr) }
    }

    /// Creates a new, empty packed pointer.
    pub const fn null() -> Self {
        let _ = Self::_ASSERT;
        Self { ptr: core::ptr::null_mut() }
    }

    /// Creates a `PackedPointer` from a pointer with zeroed data bits.
    pub fn from_ptr(ptr: *mut T) -> Self {
        Self::new(ptr, 0)
    }

    /// Creates a `PackedPointer` with a null pointer and specified data.
    ///
    /// # Panics
    ///
    /// Panics if the data exceeds the allowed number of bits.
    pub const fn from_data(data: usize) -> Self {
        let _ = Self::_ASSERT;
        assert!(data & Self::PTR_MASK == 0, "Data exceeds allowed bits");

        Self { ptr: (data & Self::DATA_MASK) as *mut T }
    }

    /// Returns the unpacked pointer, preserving its original provenance.
    pub fn ptr(&self) -> *mut T {
        self.ptr.map_addr(|addr| addr & Self::PTR_MASK)
    }

    /// Returns the unpacked data.
    pub fn data(&self) -> usize {
        self.ptr.addr() & Self::DATA_MASK
    }

    /// Sets the pointer, preserving the currently packed data.
    pub fn set_ptr(&mut self, ptr: *mut T) {
        debug_assert!(
            ptr.addr() & Self::DATA_MASK == 0,
            "Pointer {:?} is not aligned to at least {} bytes",
            ptr,
            1 << DATA_BITS
        );
        let data = self.data();
        let packed_addr = (ptr.addr() & Self::PTR_MASK) | data;
        self.ptr = ptr.with_addr(packed_addr);
    }

    /// Sets the data, preserving the currently packed pointer and its provenance.
    pub fn set_data(&mut self, data: usize) {
        debug_assert!(data & Self::PTR_MASK == 0, "Data {} exceeds {} bits", data, DATA_BITS);
        let packed_addr = (self.ptr.addr() & Self::PTR_MASK) | (data & Self::DATA_MASK);
        self.ptr = self.ptr.with_addr(packed_addr);
    }

    /// Resets the packed pointer to null with zero data.
    pub fn reset(&mut self) {
        self.ptr = core::ptr::null_mut();
    }

    /// Returns `true` if the unpacked pointer is null.
    pub fn is_null(&self) -> bool {
        self.ptr().is_null()
    }
}

impl<T, const DATA_BITS: usize, const CHECK_ALIGNMENT: bool> Default
    for PackedPointer<T, DATA_BITS, CHECK_ALIGNMENT>
{
    fn default() -> Self {
        Self::null()
    }
}

impl<T, const DATA_BITS: usize, const CHECK_ALIGNMENT: bool> From<*mut T>
    for PackedPointer<T, DATA_BITS, CHECK_ALIGNMENT>
{
    fn from(ptr: *mut T) -> Self {
        Self::from_ptr(ptr)
    }
}

impl<T, const DATA_BITS: usize, const CHECK_ALIGNMENT: bool> core::fmt::Debug
    for PackedPointer<T, DATA_BITS, CHECK_ALIGNMENT>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PackedPointer")
            .field("ptr", &self.ptr())
            .field("data", &self.data())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    #[repr(align(8))]
    struct Align8(#[allow(dead_code)] u64);

    #[test]
    fn test_basic_pack_unpack() {
        let mut val = Align8(42);
        let raw_ptr = &mut val as *mut Align8;

        let packed = PackedPointer::<Align8, 3>::new(raw_ptr, 5);
        assert_eq!(packed.ptr(), raw_ptr);
        assert_eq!(packed.data(), 5);
        assert!(!packed.is_null());

        unsafe {
            assert_eq!((*packed.ptr()).0, 42);
        }
    }

    #[test]
    fn test_setters() {
        let mut val1 = Align8(10);
        let mut val2 = Align8(20);
        let raw_ptr1 = &mut val1 as *mut Align8;
        let raw_ptr2 = &mut val2 as *mut Align8;

        let mut packed = PackedPointer::<Align8, 3>::from_ptr(raw_ptr1);
        assert_eq!(packed.ptr(), raw_ptr1);
        assert_eq!(packed.data(), 0);

        packed.set_data(7);
        assert_eq!(packed.ptr(), raw_ptr1);
        assert_eq!(packed.data(), 7);

        packed.set_ptr(raw_ptr2);
        assert_eq!(packed.ptr(), raw_ptr2);
        assert_eq!(packed.data(), 7);

        packed.reset();
        assert!(packed.is_null());
        assert_eq!(packed.data(), 0);
    }

    #[test]
    fn test_default() {
        let packed = PackedPointer::<Align8, 3>::default();
        assert!(packed.is_null());
        assert_eq!(packed.data(), 0);
    }

    #[test]
    fn test_const_constructors() {
        const MY_NULL_PTR: PackedPointer<Align8, 3> = PackedPointer::null();
        const MY_DATA_PTR: PackedPointer<Align8, 3> = PackedPointer::from_data(5);

        assert!(MY_NULL_PTR.is_null());
        assert_eq!(MY_NULL_PTR.data(), 0);

        assert!(MY_DATA_PTR.is_null());
        assert_eq!(MY_DATA_PTR.data(), 5);
    }

    #[test]
    fn test_pointer_deref() {
        let mut val = Align8(42);
        let packed = PackedPointer::<Align8, 3>::from_ptr(&mut val);
        unsafe {
            assert_eq!((*packed.ptr()).0, 42);
            (*packed.ptr()).0 = 100;
        }
        assert_eq!(val.0, 100);
    }

    #[test]
    fn test_comparisons() {
        let mut val1 = Align8(10);
        let mut val2 = Align8(20);
        let raw_ptr1 = &mut val1 as *mut Align8;
        let raw_ptr2 = &mut val2 as *mut Align8;

        let ptr1 = PackedPointer::<Align8, 3>::new(raw_ptr1, 1);
        let ptr1_again = PackedPointer::<Align8, 3>::new(raw_ptr1, 1);
        let ptr1_diff_data = PackedPointer::<Align8, 3>::new(raw_ptr1, 2);
        let ptr2 = PackedPointer::<Align8, 3>::new(raw_ptr2, 1);

        assert_eq!(ptr1, ptr1_again);
        assert_ne!(ptr1, ptr1_diff_data);
        assert_ne!(ptr1, ptr2);

        let null_ptr = PackedPointer::<Align8, 3>::default();
        assert_eq!(null_ptr, core::ptr::null_mut());
        assert_ne!(ptr1, core::ptr::null_mut());
    }

    #[test]
    fn test_disabled_alignment_check() {
        #[derive(Debug)]
        #[repr(align(4))]
        struct Align4(#[allow(dead_code)] u32);

        // Align4 only has 4-byte alignment (2 bits), but we request 3 bits (8-byte alignment
        // requirement).  This compiles because CHECK_ALIGNMENT is false.

        // We must ensure that the actual pointer used at runtime is 8-byte aligned
        // if we want to avoid panicking inside debug_assert.
        // Let's allocate an 8-byte aligned buffer and cast a pointer to it.
        #[repr(align(8))]
        struct Align8Buffer(#[allow(dead_code)] [u8; 8]);
        let mut buffer = Align8Buffer([0; 8]);
        let raw_ptr = &mut buffer as *mut Align8Buffer as *mut Align4;

        let packed = PackedPointer::<Align4, 3, false>::new(raw_ptr, 5);
        assert_eq!(packed.ptr(), raw_ptr);
        assert_eq!(packed.data(), 5);
    }

    #[test]
    fn test_packed_pointer_clone() {
        let mut val = Align8(42);
        let raw_ptr = &mut val as *mut Align8;
        let packed = PackedPointer::<Align8, 3>::new(raw_ptr, 5);
        let cloned = packed.clone();
        assert_eq!(cloned, packed);
    }

    #[test]
    fn test_packed_pointer_partial_eq_const_ptr() {
        let mut val = Align8(42);
        let raw_ptr = &mut val as *mut Align8;
        let packed = PackedPointer::<Align8, 3>::new(raw_ptr, 5);
        let const_ptr: *const Align8 = raw_ptr as *const Align8;
        assert!(packed == const_ptr);
    }

    #[test]
    fn test_packed_pointer_from_mut_ptr() {
        let mut val = Align8(42);
        let raw_ptr = &mut val as *mut Align8;
        let from_ptr = PackedPointer::<Align8, 3>::from(raw_ptr);
        assert_eq!(from_ptr.ptr(), raw_ptr);
        assert_eq!(from_ptr.data(), 0);
    }

    #[test]
    fn test_packed_pointer_debug_fmt() {
        extern crate alloc;
        let mut val = Align8(42);
        let raw_ptr = &mut val as *mut Align8;
        let packed = PackedPointer::<Align8, 3>::new(raw_ptr, 5);
        let debug_str = alloc::format!("{:?}", packed);
        assert!(debug_str.contains("PackedPointer"));
        assert!(debug_str.contains("ptr"));
        assert!(debug_str.contains("data"));
    }

    #[test]
    fn test_packed_pointer_from_data() {
        let from_d = PackedPointer::<Align8, 3>::from_data(6);
        assert!(from_d.is_null());
        assert_eq!(from_d.data(), 6);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "is not aligned")]
    fn test_unaligned_pointer_panics() {
        let mut val = Align8(42);
        let raw_ptr = &mut val as *mut Align8;
        let unaligned_ptr = raw_ptr.with_addr(raw_ptr.addr() | 1);
        let _ = PackedPointer::<Align8, 3>::new(unaligned_ptr, 0);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "is not aligned")]
    fn test_unaligned_set_ptr_panics() {
        let mut val = Align8(42);
        let mut packed = PackedPointer::<Align8, 3>::from_ptr(&mut val);
        let raw_ptr = &mut val as *mut Align8;
        let unaligned_ptr = raw_ptr.with_addr(raw_ptr.addr() | 1);
        packed.set_ptr(unaligned_ptr);
    }
}
