// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ops::{Deref, DerefMut};
use core::ptr::slice_from_raw_parts_mut;
use core::slice::from_raw_parts;

use munge::{Destructure, Move, Restructure};
use zerocopy::{FromBytes, IntoBytes};

/// An initialized but potentially invalid value.
///
/// The bytes of a `Slot` are always valid to read, but may not represent a
/// valid value of its type. For example, a `Slot<'_, bool>` may not be set to
/// 0 or 1.
#[repr(transparent)]
pub struct Slot<'de, T: ?Sized> {
    ptr: *mut T,
    _phantom: PhantomData<&'de mut [u8]>,
}

// SAFETY: `Slot` represents exclusive ownership of a `T`, so it is `Send` if `T` is `Send`.
unsafe impl<T: Send> Send for Slot<'_, T> {}
// SAFETY: `Slot` represents exclusive ownership of a `T`, so it is `Sync` if `T` is `Sync`.
unsafe impl<T: Sync> Sync for Slot<'_, T> {}

impl<'de, T: ?Sized> Slot<'de, T> {
    /// Returns a new `Slot` backed by the given `MaybeUninit`.
    pub fn new(backing: &'de mut MaybeUninit<T>) -> Self
    where
        T: Sized,
    {
        // SAFETY: `backing` is a valid mutable reference, so its pointer is valid for writes.
        unsafe {
            backing.as_mut_ptr().write_bytes(0, 1);
        }
        // SAFETY: The memory has been initialized to zero by the previous `write_bytes` call.
        unsafe { Self::new_unchecked(backing.as_mut_ptr()) }
    }

    /// Creates a new slot from the given pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must point to enough initialized bytes with the correct alignment
    /// to represent a `T`.
    pub unsafe fn new_unchecked(ptr: *mut T) -> Self {
        Self { ptr, _phantom: PhantomData }
    }

    /// Mutably reborrows the slot.
    pub fn as_mut(&mut self) -> Slot<'_, T> {
        Self { ptr: self.ptr, _phantom: PhantomData }
    }

    /// Returns a mutable pointer to the underlying potentially-invalid value.
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr
    }

    /// Returns a pointer to the underlying potentially-invalid value.
    pub fn as_ptr(&self) -> *const T {
        self.ptr
    }

    /// Returns a reference to the contained value.
    ///
    /// # Safety
    ///
    /// The slot must contain a valid `T`.
    pub unsafe fn deref_unchecked(&self) -> &T {
        // SAFETY: The caller guarantees that the slot contains a valid `T`.
        unsafe { &*self.as_ptr() }
    }

    /// Returns a mutable reference to the contained value.
    ///
    /// # Safety
    ///
    /// The slot must contain a valid `T`.
    pub unsafe fn deref_mut_unchecked(&mut self) -> &mut T {
        // SAFETY: The caller guarantees that the slot contains a valid `T`.
        unsafe { &mut *self.as_mut_ptr() }
    }

    /// Writes the given value into the slot.
    pub fn write(&mut self, value: T)
    where
        T: IntoBytes + Sized,
    {
        // SAFETY: `self.ptr` is valid for writes.
        unsafe {
            self.as_mut_ptr().write(value);
        }
    }
}

impl<'de, T: Sized> Slot<'de, T> {
    /// Creates a new slot from the given backing storage.
    ///
    /// # Safety
    ///
    /// `backing` must actually be initialized with valid `T`.
    pub unsafe fn new_unchecked_from_maybe_uninit(backing: &mut MaybeUninit<T>) -> Self {
        Self { ptr: backing.as_mut_ptr(), _phantom: PhantomData }
    }
}

impl<T> Slot<'_, T> {
    /// Returns a slice of the underlying bytes.
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: `self.ptr` is valid for reads of `size_of::<T>()` bytes because it
        // points to initialized memory.
        unsafe { from_raw_parts(self.ptr.cast::<u8>(), size_of::<T>()) }
    }
}

impl<T, const N: usize> Slot<'_, [T; N]> {
    /// Returns a slot of the element at the given index.
    pub fn index(&mut self, index: usize) -> Slot<'_, T> {
        assert!(index < N, "attempted to index out-of-bounds");

        // SAFETY: `index` is checked to be within bounds, so the calculated pointer is valid.
        Slot { ptr: unsafe { self.as_mut_ptr().cast::<T>().add(index) }, _phantom: PhantomData }
    }
}

impl<T> Slot<'_, [T]> {
    /// Creates a new slice slot from the given pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must point to enough initialized bytes with the correct alignment
    /// to represent a slice of `len` `T`s.
    pub unsafe fn new_slice_unchecked(ptr: *mut T, len: usize) -> Self {
        Self { ptr: slice_from_raw_parts_mut(ptr, len), _phantom: PhantomData }
    }

    /// Returns a slot of the element at the given index.
    pub fn index(&mut self, index: usize) -> Slot<'_, T> {
        assert!(index < self.ptr.len(), "attempted to index out-of-bounds");

        // SAFETY: `index` is checked to be within bounds of the slice, so the calculated pointer
        // is valid.
        Slot { ptr: unsafe { self.as_mut_ptr().cast::<T>().add(index) }, _phantom: PhantomData }
    }
}

impl<T: FromBytes> Deref for Slot<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: `T` implements `FromBytes`, so any initialized byte sequence represents a valid
        // `T`.
        unsafe { &*self.as_ptr() }
    }
}

impl<T: FromBytes> DerefMut for Slot<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: `T` implements `FromBytes`, so any initialized byte sequence represents a valid
        // `T`.
        unsafe { &mut *self.as_mut_ptr() }
    }
}

impl<'de, T> Iterator for Slot<'de, [T]> {
    type Item = Slot<'de, T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.ptr.len() == 0 {
            return None;
        }

        let result = Slot { ptr: self.ptr.cast::<T>(), _phantom: PhantomData };

        self.ptr =
            // SAFETY: The slice has at least one element, so advancing the pointer by 1 is safe.
            slice_from_raw_parts_mut(unsafe { self.ptr.cast::<T>().add(1) }, self.ptr.len() - 1);

        Some(result)
    }
}

// SAFETY: `underlying` returns a pointer to the wrapped data, which is valid and uniquely owned by
// the `Slot`.
unsafe impl<T> Destructure for Slot<'_, T> {
    type Underlying = T;
    type Destructuring = Move;

    fn underlying(&mut self) -> *mut Self::Underlying {
        self.as_mut_ptr()
    }
}

// SAFETY: `restructure` is called with a pointer to a subfield of `T` which is guaranteed to be
// initialized and have the correct lifetime.
unsafe impl<'de, T, U: 'de> Restructure<U> for Slot<'de, T> {
    type Restructured = Slot<'de, U>;

    unsafe fn restructure(&self, ptr: *mut U) -> Self::Restructured {
        Slot { ptr, _phantom: PhantomData }
    }
}
