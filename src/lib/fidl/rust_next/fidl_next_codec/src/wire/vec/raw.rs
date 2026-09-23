// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::MaybeUninit;
use core::ptr::slice_from_raw_parts_mut;

use munge::munge;

use crate::{Constrained, Slot, ValidationError, Wire, wire};

#[repr(C)]
pub struct RawVector<'de, T> {
    pub len: wire::Uint64,
    pub ptr: wire::Pointer<'de, T>,
}

impl<T> Constrained for RawVector<'_, T> {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

// SAFETY: `RawVector` is `repr(C)` and contains only `Wire` types (`wire::Uint64` and
// `wire::Pointer`). It has no padding bytes, and lifetime erasure is safe since `RawVector` is
// covariant over its lifetime.
unsafe impl<T: Wire> Wire for RawVector<'static, T> {
    type Narrowed<'de> = RawVector<'de, T::Narrowed<'de>>;

    #[inline]
    fn zero_padding(_: &mut MaybeUninit<Self>) {
        // Wire vectors have no padding bytes
    }
}

// SAFETY: `RawWireVector` doesn't add any restrictions on sending across thread boundaries, and so
// is `Send` if `T` is `Send`.
unsafe impl<T: Send> Send for RawVector<'_, T> {}

// SAFETY: `RawWireVector` doesn't add any interior mutability, so it is `Sync` if `T` is `Sync`.
unsafe impl<T: Sync> Sync for RawVector<'_, T> {}

impl<T> RawVector<'_, T> {
    pub fn encode_present(out: &mut MaybeUninit<Self>, len: u64) {
        munge!(let Self { len: encoded_len, ptr } = out);
        encoded_len.write(wire::Uint64(len));
        wire::Pointer::encode_present(ptr);
    }

    pub fn encode_absent(out: &mut MaybeUninit<Self>) {
        munge!(let Self { len, ptr } = out);
        len.write(wire::Uint64(0));
        wire::Pointer::encode_absent(ptr);
    }

    pub fn len(&self) -> u64 {
        *self.len
    }

    pub fn as_ptr(&self) -> *mut T {
        self.ptr.as_ptr()
    }

    pub fn as_slice_ptr(&self) -> *mut [T] {
        slice_from_raw_parts_mut(self.as_ptr(), self.len() as usize)
    }
}
