// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ops::{Deref, DerefMut};

// This is a wrapper around externally allocated pointers. In order to allow
// these to be passed to a separate thread but still maintain some more well-
// defined lifetime bounds, we use this wrapper type.
pub struct ExtBuffer<T> {
    inner: *mut T,
    len: usize,
}

impl<T> ExtBuffer<T> {
    /// # Safety
    ///
    /// Full requirements are defined at [std::slice::from_raw_parts_mut], as this is the most
    /// restrictively used function internally. Generally speaking, though:
    ///
    /// - `ptr` must point to a contiguous block of memory aligned to `T` of length
    ///   `mem::size_of::<T>() * len` bytes.
    /// - The contiguous memory space that `ptr` must point to must be a single allocated object.
    /// - Each object `T` must be properly initialized.
    /// - `ptr` must not be aliased with any other live pointers during the lifetime of this
    ///   struct.
    /// - `ptr` must not be null even if `len` is `0`.
    /// - `ptr` must be [valid] for reads and writes.
    /// - The data pointed to by `ptr` must outlive this struct.
    ///
    /// [valid]: std::ptr#safety
    pub unsafe fn new(ptr: *mut T, len: usize) -> Self {
        Self { inner: ptr, len }
    }
}

impl<T: Send + Sized> Deref for ExtBuffer<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.inner, self.len) }
    }
}

impl<T: Send + Sized> DerefMut for ExtBuffer<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.inner, self.len) }
    }
}

unsafe impl<T: Send> Send for ExtBuffer<T> {}
