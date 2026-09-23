// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::user_ptr::{UserInOutPtr, UserInPtr, UserOutPtr};
use crate::arch_rs::arch_copy_from_user;
use core::mem::{MaybeUninit, size_of};
use zerocopy::{FromBytes, Immutable, IntoBytes};
use zx_status::Status;
use zx_types::zx_iovec_t;

#[repr(C)]
#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, Immutable, Default)]
struct RawIovec {
    buffer: usize,
    capacity: usize,
}

/// A copy of a user-provided `zx_iovec_t` vector entry for read operations.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct UserInVector {
    pub data: UserInPtr<u8>,
    pub len: usize,
}

/// A copy of a user-provided `zx_iovec_t` vector entry for write operations.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct UserOutVector {
    pub data: UserOutPtr<u8>,
    pub len: usize,
}

/// A copy of a user-provided `zx_iovec_t` vector entry for read-write operations.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default)]
pub struct UserInOutVector {
    pub data: UserInOutPtr<u8>,
    pub len: usize,
}

const _: () = {
    assert!(size_of::<UserInVector>() == size_of::<zx_iovec_t>());
    assert!(core::mem::align_of::<UserInVector>() == core::mem::align_of::<zx_iovec_t>());
    assert!(size_of::<UserOutVector>() == size_of::<zx_iovec_t>());
    assert!(core::mem::align_of::<UserOutVector>() == core::mem::align_of::<zx_iovec_t>());
    assert!(size_of::<UserInOutVector>() == size_of::<zx_iovec_t>());
    assert!(core::mem::align_of::<UserInOutVector>() == core::mem::align_of::<zx_iovec_t>());
};

fn get_total_capacity(vector: UserInPtr<zx_iovec_t>, count: usize) -> Result<usize, Status> {
    let mut total = 0usize;
    for_each(vector, count, |_buffer, capacity| {
        total = total.checked_add(capacity).ok_or(Status::INVALID_ARGS)?;
        Ok(())
    })?;
    Ok(total)
}

fn copy_to_slice<T>(
    vector: UserInPtr<zx_iovec_t>,
    count: usize,
    out: &mut [MaybeUninit<T>],
) -> Result<&mut [T], Status> {
    if count > out.len() {
        return Err(Status::INVALID_ARGS);
    }
    if count == 0 {
        return Ok(&mut []);
    }
    let bytes_to_copy = count.checked_mul(size_of::<T>()).ok_or(Status::INVALID_ARGS)?;
    // SAFETY: `out` has at least `count` elements, so `out.as_mut_ptr()` has enough space for
    // `bytes_to_copy`. `T` is layout-compatible with `zx_iovec_t`, and any bit pattern is valid
    // for its pointer and length fields. On success, `count` elements are initialized.
    unsafe {
        arch_copy_from_user(
            out.as_mut_ptr() as *mut core::ffi::c_void,
            vector.as_ptr() as *const core::ffi::c_void,
            bytes_to_copy,
        )?;
        Ok(core::slice::from_raw_parts_mut(out.as_mut_ptr() as *mut T, count))
    }
}

fn for_each<F>(vector: UserInPtr<zx_iovec_t>, count: usize, mut cb: F) -> Result<(), Status>
where
    F: FnMut(usize, usize) -> Result<(), Status>,
{
    let raw_vec = vector.reinterpret::<RawIovec>();
    for i in 0..count {
        let elem = raw_vec.element_offset(i).read()?;
        cb(elem.buffer, elem.capacity)?;
    }
    Ok(())
}

/// A wrapper around a userspace array of `zx_iovec_t` for read operations.
#[derive(Debug, Copy, Clone)]
pub struct UserInIovec {
    vector: UserInPtr<zx_iovec_t>,
    count: usize,
}

impl UserInIovec {
    /// Constructs a new `UserInIovec`.
    pub const fn new(vector: UserInPtr<zx_iovec_t>, count: usize) -> Self {
        Self { vector, count }
    }

    /// Returns the underlying pointer to the user `zx_iovec_t` array.
    pub const fn vector(&self) -> UserInPtr<zx_iovec_t> {
        self.vector
    }

    /// Returns the number of iovec structures in the array.
    pub const fn count(&self) -> usize {
        self.count
    }

    /// Returns true if the underlying pointer is null.
    pub fn is_null(&self) -> bool {
        self.vector.is_null()
    }

    /// Calculates the total capacity across all iovecs.
    pub fn get_total_capacity(&self) -> Result<usize, Status> {
        get_total_capacity(self.vector, self.count)
    }

    /// Copies the user-provided iovecs into `out`.
    ///
    /// Returns a mutable slice of initialized elements on success.
    pub fn copy_to_slice<'a>(
        &self,
        out: &'a mut [MaybeUninit<UserInVector>],
    ) -> Result<&'a mut [UserInVector], Status> {
        copy_to_slice(self.vector, self.count, out)
    }

    /// Iterates through the iovecs and invokes the callback for each user pointer and capacity.
    pub fn for_each<F>(&self, mut cb: F) -> Result<(), Status>
    where
        F: FnMut(UserInPtr<u8>, usize) -> Result<(), Status>,
    {
        for_each(self.vector, self.count, |buffer, capacity| {
            cb(UserInPtr::new(buffer as *const u8), capacity)
        })
    }
}

/// A wrapper around a userspace array of `zx_iovec_t` for write operations.
#[derive(Debug, Copy, Clone)]
pub struct UserOutIovec {
    vector: UserInPtr<zx_iovec_t>,
    count: usize,
}

impl UserOutIovec {
    /// Constructs a new `UserOutIovec`.
    pub const fn new(vector: UserInPtr<zx_iovec_t>, count: usize) -> Self {
        Self { vector, count }
    }

    /// Returns the underlying pointer to the user `zx_iovec_t` array as a `UserOutPtr`.
    pub fn as_user_out_ptr(&self) -> UserOutPtr<zx_iovec_t> {
        UserOutPtr::new(self.vector.as_ptr() as *mut zx_iovec_t)
    }

    /// Returns the underlying pointer to the user `zx_iovec_t` array.
    pub const fn vector(&self) -> UserInPtr<zx_iovec_t> {
        self.vector
    }

    /// Returns the number of iovec structures in the array.
    pub const fn count(&self) -> usize {
        self.count
    }

    /// Returns true if the underlying pointer is null.
    pub fn is_null(&self) -> bool {
        self.vector.is_null()
    }

    /// Calculates the total capacity across all iovecs.
    pub fn get_total_capacity(&self) -> Result<usize, Status> {
        get_total_capacity(self.vector, self.count)
    }

    /// Copies the user-provided iovecs into `out`.
    ///
    /// Returns a mutable slice of initialized elements on success.
    pub fn copy_to_slice<'a>(
        &self,
        out: &'a mut [MaybeUninit<UserOutVector>],
    ) -> Result<&'a mut [UserOutVector], Status> {
        copy_to_slice(self.vector, self.count, out)
    }

    /// Iterates through the iovecs and invokes the callback for each user pointer and capacity.
    pub fn for_each<F>(&self, mut cb: F) -> Result<(), Status>
    where
        F: FnMut(UserOutPtr<u8>, usize) -> Result<(), Status>,
    {
        for_each(self.vector, self.count, |buffer, capacity| {
            cb(UserOutPtr::new(buffer as *mut u8), capacity)
        })
    }
}

/// Constructs a `UserOutIovec` from a user out-pointer and count.
pub fn make_user_out_iovec(vector: UserOutPtr<zx_iovec_t>, count: usize) -> UserOutIovec {
    UserOutIovec::new(UserInPtr::new(vector.as_ptr() as *const zx_iovec_t), count)
}

/// Constructs a `UserInIovec` from a user in-pointer and count.
pub fn make_user_in_iovec(vector: UserInPtr<zx_iovec_t>, count: usize) -> UserInIovec {
    UserInIovec::new(vector, count)
}

/// A wrapper around a userspace array of `zx_iovec_t` for read-write operations.
#[derive(Debug, Copy, Clone)]
pub struct UserInOutIovec {
    vector: UserInPtr<zx_iovec_t>,
    count: usize,
}

impl UserInOutIovec {
    /// Constructs a new `UserInOutIovec`.
    pub const fn new(vector: UserInPtr<zx_iovec_t>, count: usize) -> Self {
        Self { vector, count }
    }

    /// Returns true if the underlying pointer is null.
    pub fn is_null(&self) -> bool {
        self.vector.is_null()
    }

    /// Calculates the total capacity across all iovecs.
    pub fn get_total_capacity(&self) -> Result<usize, Status> {
        get_total_capacity(self.vector, self.count)
    }

    /// Copies the user-provided iovecs into `out`.
    ///
    /// Returns a mutable slice of initialized elements on success.
    pub fn copy_to_slice<'a>(
        &self,
        out: &'a mut [MaybeUninit<UserInOutVector>],
    ) -> Result<&'a mut [UserInOutVector], Status> {
        copy_to_slice(self.vector, self.count, out)
    }

    /// Iterates through the iovecs and invokes the callback for each user pointer and capacity.
    pub fn for_each<F>(&self, mut cb: F) -> Result<(), Status>
    where
        F: FnMut(UserInOutPtr<u8>, usize) -> Result<(), Status>,
    {
        for_each(self.vector, self.count, |buffer, capacity| {
            cb(UserInOutPtr::new(buffer as *mut u8), capacity)
        })
    }
}
