// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::sys::zx_iovec_t;
use std::ops::Deref;

/// A reference to a readable slice of memory for IOB writes, analogous to `std::io::IoSlice` but
/// for Zircon IOB I/O. ABI-compatible with `zx_iovec_t`, guaranteeing the pointed-to bytes are
/// readable using lifetimes.
#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct IobIoSlice<'a>(zx_iovec_t, std::marker::PhantomData<&'a [u8]>);

impl<'a> IobIoSlice<'a> {
    /// Convert a Rust byte slice to a `IobIoSlice`. If the input slice is longer than can be
    /// referenced by a `zx_iovec_t` the length will be truncated.
    pub fn new(buf: &'a [u8]) -> Self {
        Self(zx_iovec_t { buffer: buf.as_ptr(), capacity: buf.len() }, std::marker::PhantomData)
    }
}

impl<'a> Deref for IobIoSlice<'a> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: lifetime marker guarantees these bytes are still live.
        unsafe { std::slice::from_raw_parts(self.0.buffer, self.0.capacity as usize) }
    }
}

impl<'a> std::cmp::PartialEq for IobIoSlice<'a> {
    fn eq(&self, rhs: &Self) -> bool {
        self.deref().eq(rhs.deref())
    }
}
impl<'a> std::cmp::Eq for IobIoSlice<'a> {}

impl<'a> std::hash::Hash for IobIoSlice<'a> {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.deref().hash(h)
    }
}

impl<'a> std::fmt::Debug for IobIoSlice<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.deref().fmt(f)
    }
}

// SAFETY: this type has no meaningful drop impl other than releasing its borrow.
unsafe impl<'a> Send for IobIoSlice<'a> {}

// SAFETY: this type has no mutability.
unsafe impl<'a> Sync for IobIoSlice<'a> {}
