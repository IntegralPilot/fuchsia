// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::sync::atomic::{AtomicPtr, Ordering};

/// Extension trait for references to obtain a mutable raw pointer.
pub trait ToMutPtr {
    /// The target type of the pointer.
    type Target: ?Sized;

    /// Casts the reference to a mutable raw pointer.
    fn to_mut_ptr(&self) -> *mut Self::Target;
}

impl<T: ?Sized> ToMutPtr for T {
    type Target = T;

    #[inline(always)]
    fn to_mut_ptr(&self) -> *mut T {
        self as *const T as *mut T
    }
}

/// An atomic pointer type which operates on `*const T`.
///
/// This type provides a typed wrapper around [`AtomicPtr`] that enforces `*const T`
/// invariants throughout its API, avoiding the need for `cast_mut()` or manual
/// pointer qualification casts at call sites.
#[repr(transparent)]
pub struct AtomicConstPtr<T> {
    inner: AtomicPtr<T>,
}

impl<T> AtomicConstPtr<T> {
    /// Creates a new `AtomicConstPtr` initialized with `ptr`.
    #[inline]
    pub const fn new(ptr: *const T) -> Self {
        Self { inner: AtomicPtr::new(ptr.cast_mut()) }
    }

    /// Loads a value from the pointer with the specified memory ordering.
    #[inline]
    pub fn load(&self, order: Ordering) -> *const T {
        self.inner.load(order).cast_const()
    }

    /// Stores a value into the pointer with the specified memory ordering.
    #[inline]
    pub fn store(&self, ptr: *const T, order: Ordering) {
        self.inner.store(ptr.cast_mut(), order);
    }

    /// Stores a value into the pointer, returning the previous value.
    #[inline]
    pub fn swap(&self, ptr: *const T, order: Ordering) -> *const T {
        self.inner.swap(ptr.cast_mut(), order).cast_const()
    }

    /// Stores a value into the pointer if the current value is the same as `current`.
    ///
    /// The return value is a result indicating whether the new value was written and
    /// containing the previous value.
    #[inline]
    pub fn compare_exchange(
        &self,
        current: *const T,
        new: *const T,
        success: Ordering,
        failure: Ordering,
    ) -> Result<*const T, *const T> {
        self.inner
            .compare_exchange(current.cast_mut(), new.cast_mut(), success, failure)
            .map(|p| p.cast_const())
            .map_err(|p| p.cast_const())
    }

    /// Stores a value into the pointer if the current value is the same as `current`.
    ///
    /// Unlike [`compare_exchange`], this function is allowed to spuriously fail.
    #[inline]
    pub fn compare_exchange_weak(
        &self,
        current: *const T,
        new: *const T,
        success: Ordering,
        failure: Ordering,
    ) -> Result<*const T, *const T> {
        self.inner
            .compare_exchange_weak(current.cast_mut(), new.cast_mut(), success, failure)
            .map(|p| p.cast_const())
            .map_err(|p| p.cast_const())
    }

    /// Fetches the value, and applies a function to it that returns an optional new value. Returns
    /// a `Result` of `Ok(previous_value)` if the function returned `Some(_)`, else
    /// `Err(previous_value)`.
    #[inline]
    pub fn try_update<F>(
        &self,
        set_order: Ordering,
        fetch_order: Ordering,
        mut f: F,
    ) -> Result<*const T, *const T>
    where
        F: FnMut(*const T) -> Option<*const T>,
    {
        self.inner
            .try_update(set_order, fetch_order, |p| f(p.cast_const()).map(|c| c.cast_mut()))
            .map(|p| p.cast_const())
            .map_err(|p| p.cast_const())
    }

    /// Consumes the atomic pointer, returning the underlying raw pointer.
    #[inline]
    pub fn into_inner(self) -> *const T {
        self.inner.into_inner().cast_const()
    }
}

impl<T> core::fmt::Debug for AtomicConstPtr<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("AtomicConstPtr").field(&self.load(Ordering::Relaxed)).finish()
    }
}

impl<T> core::fmt::Pointer for AtomicConstPtr<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Pointer::fmt(&self.load(Ordering::Relaxed), f)
    }
}

impl<T> Default for AtomicConstPtr<T> {
    #[inline]
    fn default() -> Self {
        Self::new(core::ptr::null())
    }
}

impl<T> From<*const T> for AtomicConstPtr<T> {
    #[inline]
    fn from(ptr: *const T) -> Self {
        Self::new(ptr)
    }
}

/// Forms a slice from an FFI pointer and a length, returning an empty slice `&[]` when `len == 0`
/// even if `data` is null or misaligned.
///
/// # Safety
///
/// If `len > 0`, `data` must satisfy the safety requirements of [`core::slice::from_raw_parts`]:
/// it must be non-null, valid for reads for `len * mem::size_of::<T>()` many bytes, properly
/// aligned, and point to `len` consecutive properly initialized values of type `T` that are not
/// mutated for the duration of lifetime `'a`.
#[inline]
pub const unsafe fn slice_from_raw_parts<'a, T>(data: *const T, len: usize) -> &'a [T] {
    if len == 0 {
        &[]
    } else {
        // SAFETY: `len > 0`, and the caller guarantees `data` is valid for `len` elements.
        unsafe { core::slice::from_raw_parts(data, len) }
    }
}

/// Performs the same functionality as [`slice_from_raw_parts`], except that a mutable slice is
/// returned.
///
/// # Safety
///
/// If `len > 0`, `data` must satisfy the safety requirements of
/// [`core::slice::from_raw_parts_mut`]: it must be non-null, valid for both reads and writes for
/// `len * mem::size_of::<T>()` many bytes, properly aligned, and point to `len` consecutive
/// properly initialized values of type `T` not accessed through any other pointer for the duration
/// of lifetime `'a`.
#[inline]
pub const unsafe fn slice_from_raw_parts_mut<'a, T>(data: *mut T, len: usize) -> &'a mut [T] {
    if len == 0 {
        &mut []
    } else {
        // SAFETY: `len > 0`, and the caller guarantees `data` is uniquely valid for `len` elements.
        unsafe { core::slice::from_raw_parts_mut(data, len) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slice_from_raw_parts_zero_length() {
        let empty: &[u32] = unsafe { slice_from_raw_parts(core::ptr::null(), 0) };
        assert!(empty.is_empty());

        let empty_mut: &mut [u32] = unsafe { slice_from_raw_parts_mut(core::ptr::null_mut(), 0) };
        assert!(empty_mut.is_empty());
    }

    #[test]
    fn test_slice_from_raw_parts_non_empty() {
        let mut arr = [10u32, 20, 30];
        let s = unsafe { slice_from_raw_parts(arr.as_ptr(), arr.len()) };
        assert_eq!(s, &[10, 20, 30]);

        let s_mut = unsafe { slice_from_raw_parts_mut(arr.as_mut_ptr(), arr.len()) };
        s_mut[1] = 25;
        assert_eq!(arr, [10, 25, 30]);
    }

    #[test]
    fn test_atomic_const_ptr_basic() {
        static DUMMY: u32 = 42;
        static DUMMY2: u32 = 99;

        let ptr = AtomicConstPtr::new(&DUMMY as *const u32);
        assert_eq!(ptr.load(Ordering::SeqCst), &DUMMY as *const u32);

        ptr.store(&DUMMY2 as *const u32, Ordering::SeqCst);
        assert_eq!(ptr.load(Ordering::SeqCst), &DUMMY2 as *const u32);

        let old = ptr.swap(&DUMMY as *const u32, Ordering::SeqCst);
        assert_eq!(old, &DUMMY2 as *const u32);
        assert_eq!(ptr.load(Ordering::SeqCst), &DUMMY as *const u32);

        let res = ptr.compare_exchange(
            &DUMMY as *const u32,
            &DUMMY2 as *const u32,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        assert_eq!(res, Ok(&DUMMY as *const u32));
        assert_eq!(ptr.load(Ordering::SeqCst), &DUMMY2 as *const u32);

        let update_res = ptr.try_update(Ordering::SeqCst, Ordering::SeqCst, |p| {
            if p == &DUMMY2 as *const u32 { Some(&DUMMY as *const u32) } else { None }
        });
        assert_eq!(update_res, Ok(&DUMMY2 as *const u32));
        assert_eq!(ptr.load(Ordering::SeqCst), &DUMMY as *const u32);

        let default_ptr: AtomicConstPtr<u32> = AtomicConstPtr::default();
        assert!(default_ptr.load(Ordering::Relaxed).is_null());
    }
}
