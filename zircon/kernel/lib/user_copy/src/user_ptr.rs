// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::arch_rs::{arch_copy_from_user, arch_copy_to_user};
use core::mem::MaybeUninit;
use zerocopy::{FromBytes, Immutable, IntoBytes};
use zx_status::Status;

/// A wrapper around a const pointer to user memory.
#[repr(transparent)]
#[derive(Debug)]
pub struct UserInPtr<T> {
    ptr: *const T,
}

impl<T> Copy for UserInPtr<T> {}

impl<T> Clone for UserInPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Default for UserInPtr<T> {
    fn default() -> Self {
        Self { ptr: core::ptr::null() }
    }
}

impl<T> UserInPtr<T> {
    /// Constructs a new `UserInPtr`.
    pub const fn new(ptr: *const T) -> Self {
        Self { ptr }
    }

    /// Returns true if the underlying pointer is null.
    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    /// Returns the underlying raw pointer.
    pub fn as_ptr(&self) -> *const T {
        self.ptr
    }

    /// Returns a pointer offset by `count` bytes.
    pub fn byte_offset(&self, count: isize) -> Self {
        if self.ptr.is_null() {
            return Self::new(core::ptr::null());
        }
        Self::new(self.ptr.wrapping_byte_offset(count))
    }

    /// Returns a pointer offset by `index` elements.
    pub fn element_offset(&self, index: usize) -> Self {
        if self.ptr.is_null() {
            return Self::new(core::ptr::null());
        }
        Self::new(self.ptr.wrapping_add(index))
    }

    /// Reinterprets the pointer as a pointer to type `U`.
    pub fn reinterpret<U>(&self) -> UserInPtr<U> {
        UserInPtr::new(self.ptr.cast::<U>())
    }

    /// Copies a single element from userspace into `dst`.
    pub fn copy_from_user<'a>(&self, dst: &'a mut MaybeUninit<T>) -> Result<&'a mut T, Status>
    where
        T: FromBytes,
    {
        // SAFETY: `dst.as_mut_ptr()` points to `size_of::<T>()` bytes of valid kernel memory.  If
        // `arch_copy_from_user` succeeds, `dst` is fully initialized with bytes from user memory.
        // Since `T: FromBytes`, any bit pattern of size `size_of::<T>()` is a valid representation
        // of `T`, making `assume_init_mut()` safe to call.
        unsafe {
            arch_copy_from_user(
                dst.as_mut_ptr() as *mut core::ffi::c_void,
                self.ptr as *const core::ffi::c_void,
                core::mem::size_of::<T>(),
            )?;
            Ok(dst.assume_init_mut())
        }
    }

    /// Reads and returns a single copyable element from userspace.
    pub fn read(&self) -> Result<T, Status>
    where
        T: FromBytes,
    {
        let mut val = MaybeUninit::uninit();
        self.copy_from_user(&mut val)?;
        // SAFETY: `copy_from_user` succeeded, so `val` has been fully initialized with a valid
        // byte representation of `T` (since `T: FromBytes`).
        Ok(unsafe { val.assume_init() })
    }

    /// Copies a slice of elements from userspace into `dst`.
    pub fn copy_slice_from_user<'a>(
        &self,
        dst: &'a mut [MaybeUninit<T>],
    ) -> Result<&'a mut [T], Status>
    where
        T: FromBytes,
    {
        let len_bytes = core::mem::size_of_val(dst);
        // SAFETY: `dst.as_mut_ptr()` points to `len_bytes` of valid kernel memory buffer space.
        // Upon successful return, all elements in `dst` are initialized with bytes from user
        // memory. Since `T: FromBytes`, converting the raw pointer to a mutable slice of `T` with
        // length `dst.len()` is safe.
        unsafe {
            arch_copy_from_user(
                dst.as_mut_ptr() as *mut core::ffi::c_void,
                self.ptr as *const core::ffi::c_void,
                len_bytes,
            )?;
            Ok(core::slice::from_raw_parts_mut(dst.as_mut_ptr() as *mut T, dst.len()))
        }
    }
}

impl UserInPtr<u8> {
    /// Copies a string from userspace into `buf`, ensuring null termination.
    ///
    /// Copies at most `min(src_len, buf.len().saturating_sub(1))` bytes from userspace,
    /// avoiding copying extra bytes beyond what can fit in the buffer.
    ///
    /// If `src_len == 0`, no bytes are read from userspace and an empty slice `&[]` is returned.
    pub fn copy_user_string<'a>(
        &self,
        src_len: usize,
        buf: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a [u8], Status> {
        if buf.is_empty() {
            return Err(Status::INVALID_ARGS);
        }

        let copy_len = src_len.min(buf.len() - 1);
        if copy_len > 0 {
            self.copy_slice_from_user(&mut buf[..copy_len]).map_err(|_| Status::INVALID_ARGS)?;
        }
        buf[copy_len].write(0);

        // SAFETY: elements 0..copy_len in `buf` have been initialized by `copy_slice_from_user`
        // (if copy_len > 0) or are empty (if copy_len == 0), and `buf[copy_len]` has been
        // initialized to 0.
        Ok(unsafe { core::slice::from_raw_parts(buf.as_ptr().cast(), copy_len) })
    }
}

/// A wrapper around a mutable pointer to user memory (write-only).
#[repr(transparent)]
#[derive(Debug)]
pub struct UserOutPtr<T> {
    ptr: *mut T,
}

impl<T> Copy for UserOutPtr<T> {}

impl<T> Clone for UserOutPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Default for UserOutPtr<T> {
    fn default() -> Self {
        Self { ptr: core::ptr::null_mut() }
    }
}

impl<T> UserOutPtr<T> {
    /// Constructs a new `UserOutPtr`.
    pub const fn new(ptr: *mut T) -> Self {
        Self { ptr }
    }

    /// Returns true if the underlying pointer is null.
    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    /// Returns the underlying raw pointer.
    pub fn as_ptr(&self) -> *mut T {
        self.ptr
    }

    /// Returns a pointer offset by `count` bytes.
    pub fn byte_offset(&self, count: isize) -> Self {
        if self.ptr.is_null() {
            return Self::new(core::ptr::null_mut());
        }
        Self::new(self.ptr.wrapping_byte_offset(count))
    }

    /// Returns a pointer offset by `index` elements.
    pub fn element_offset(&self, index: usize) -> Self {
        if self.ptr.is_null() {
            return Self::new(core::ptr::null_mut());
        }
        Self::new(self.ptr.wrapping_add(index))
    }

    /// Reinterprets the pointer as a pointer to type `U`.
    pub fn reinterpret<U>(&self) -> UserOutPtr<U> {
        UserOutPtr::new(self.ptr.cast::<U>())
    }

    /// Copies a single element from `src` to userspace.
    pub fn copy_to_user(&self, src: &T) -> Result<(), Status>
    where
        T: IntoBytes + Immutable,
    {
        let src_bytes = src.as_bytes();
        // SAFETY: `src_bytes.as_ptr()` points to `src_bytes.len()` bytes of valid kernel memory.
        unsafe {
            arch_copy_to_user(
                self.ptr as *mut core::ffi::c_void,
                src_bytes.as_ptr() as *const core::ffi::c_void,
                src_bytes.len(),
            )
        }
    }

    /// Writes a single copyable value to userspace.
    pub fn write(&self, val: T) -> Result<(), Status>
    where
        T: IntoBytes + Immutable,
    {
        self.copy_to_user(&val)
    }

    /// Copies a slice of elements from `src` to userspace.
    pub fn copy_slice_to_user(&self, src: &[T]) -> Result<(), Status>
    where
        T: IntoBytes + Immutable,
    {
        let src_bytes = src.as_bytes();
        // SAFETY: `src_bytes.as_ptr()` points to `src_bytes.len()` bytes of valid kernel memory
        // slice.
        unsafe {
            arch_copy_to_user(
                self.ptr as *mut core::ffi::c_void,
                src_bytes.as_ptr() as *const core::ffi::c_void,
                src_bytes.len(),
            )
        }
    }
}

/// A wrapper around a mutable pointer to user memory (read-write).
#[repr(transparent)]
#[derive(Debug)]
pub struct UserInOutPtr<T> {
    ptr: *mut T,
}

impl<T> Copy for UserInOutPtr<T> {}

impl<T> Clone for UserInOutPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Default for UserInOutPtr<T> {
    fn default() -> Self {
        Self { ptr: core::ptr::null_mut() }
    }
}

impl<T> UserInOutPtr<T> {
    /// Constructs a new `UserInOutPtr`.
    pub const fn new(ptr: *mut T) -> Self {
        Self { ptr }
    }

    /// Returns true if the underlying pointer is null.
    pub fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    /// Returns the underlying raw pointer.
    pub fn as_ptr(&self) -> *mut T {
        self.ptr
    }

    /// Returns a pointer offset by `count` bytes.
    pub fn byte_offset(&self, count: isize) -> Self {
        if self.ptr.is_null() {
            return Self::new(core::ptr::null_mut());
        }
        Self::new(self.ptr.wrapping_byte_offset(count))
    }

    /// Returns a pointer offset by `index` elements.
    pub fn element_offset(&self, index: usize) -> Self {
        if self.ptr.is_null() {
            return Self::new(core::ptr::null_mut());
        }
        Self::new(self.ptr.wrapping_add(index))
    }

    /// Reinterprets the pointer as a pointer to type `U`.
    pub fn reinterpret<U>(&self) -> UserInOutPtr<U> {
        UserInOutPtr::new(self.ptr.cast::<U>())
    }

    /// Returns a `UserInPtr` viewing the same user memory.
    pub const fn as_in_ptr(&self) -> UserInPtr<T> {
        UserInPtr::new(self.ptr)
    }

    /// Returns a `UserOutPtr` viewing the same user memory.
    pub const fn as_out_ptr(&self) -> UserOutPtr<T> {
        UserOutPtr::new(self.ptr)
    }

    /// Copies a single element from userspace into `dst`.
    pub fn copy_from_user<'a>(&self, dst: &'a mut MaybeUninit<T>) -> Result<&'a mut T, Status>
    where
        T: FromBytes + IntoBytes,
    {
        self.as_in_ptr().copy_from_user(dst)
    }

    /// Reads and returns a single copyable element from userspace.
    pub fn read(&self) -> Result<T, Status>
    where
        T: FromBytes + IntoBytes + Immutable,
    {
        self.as_in_ptr().read()
    }

    /// Copies a slice of elements from userspace into `dst`.
    pub fn copy_slice_from_user<'a>(
        &self,
        dst: &'a mut [MaybeUninit<T>],
    ) -> Result<&'a mut [T], Status>
    where
        T: FromBytes + IntoBytes,
    {
        self.as_in_ptr().copy_slice_from_user(dst)
    }

    /// Copies a single element from `src` to userspace.
    pub fn copy_to_user(&self, src: &T) -> Result<(), Status>
    where
        T: IntoBytes + Immutable,
    {
        self.as_out_ptr().copy_to_user(src)
    }

    /// Writes a single copyable value to userspace.
    pub fn write(&self, val: T) -> Result<(), Status>
    where
        T: IntoBytes + Immutable,
    {
        self.as_out_ptr().write(val)
    }

    /// Copies a slice of elements from `src` to userspace.
    pub fn copy_slice_to_user(&self, src: &[T]) -> Result<(), Status>
    where
        T: IntoBytes + Immutable,
    {
        self.as_out_ptr().copy_slice_to_user(src)
    }
}
