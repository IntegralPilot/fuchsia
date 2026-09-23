// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;
use core::mem::MaybeUninit;

use munge::munge;

use crate::{Chunk, DecodeError, Slot, wire};
use fidl_constants::{ALLOC_ABSENT_U64, ALLOC_PRESENT_U64};

/// A raw FIDL pointer
#[repr(C, align(8))]
pub union Pointer<'de, T> {
    encoded: wire::Uint64,
    decoded: *mut T,
    _phantom: PhantomData<&'de mut [Chunk]>,
}

// SAFETY: `Pointer` is a raw pointer wrapper and doesn't add any thread safety restrictions.
unsafe impl<T: Send> Send for Pointer<'_, T> {}
// SAFETY: `Pointer` contains no interior mutability.
unsafe impl<T: Sync> Sync for Pointer<'_, T> {}

impl<'de, T> Pointer<'de, T> {
    /// Returns whether the wire pointer was encoded present.
    pub fn is_encoded_present(slot: Slot<'_, Self>) -> Result<bool, DecodeError> {
        // `unsafe` block required in the next version of munge
        #[allow(unused_unsafe)]
        // SAFETY: `slot` is a valid `Slot` of `Pointer`. Destructuring it is safe.
        let encoded = unsafe {
            munge!(let Self { encoded } = slot);
            encoded
        };
        match **encoded {
            ALLOC_ABSENT_U64 => Ok(false),
            ALLOC_PRESENT_U64 => Ok(true),
            x => Err(DecodeError::InvalidPointerPresence(x)),
        }
    }

    /// Encodes that a pointer is present in an output.
    pub fn encode_present(out: &mut MaybeUninit<Self>) {
        // `unsafe` block required in the next version of munge
        #[allow(unused_unsafe)]
        // SAFETY: `out` is a valid mutable reference to a `MaybeUninit<Pointer>`.
        // Destructuring it via `munge!` is safe.
        let encoded = unsafe {
            munge!(let Self { encoded } = out);
            encoded
        };
        encoded.write(wire::Uint64(ALLOC_PRESENT_U64));
    }

    /// Encodes that a pointer is absent in a slot.
    pub fn encode_absent(out: &mut MaybeUninit<Self>) {
        // `unsafe` block required in the next version of munge
        #[allow(unused_unsafe)]
        // SAFETY: `out` is a valid mutable reference to a `MaybeUninit<Pointer>`.
        // Destructuring it via `munge!` is safe.
        let encoded = unsafe {
            munge!(let Self { encoded } = out);
            encoded
        };
        encoded.write(wire::Uint64(ALLOC_ABSENT_U64));
    }

    /// Sets the decoded value of the pointer.
    pub fn set_decoded(slot: Slot<'_, Self>, mut value: Slot<'de, T>) {
        // `unsafe` block required in the next version of munge
        #[allow(unused_unsafe)]
        // SAFETY: `slot` is a valid `Slot` of `Pointer`. Destructuring it is safe.
        let mut decoded = unsafe {
            munge!(let Self { decoded } = slot);
            decoded
        };
        // SAFETY: Identical to `decoded.write(ptr.into_raw())`, but raw
        // pointers don't currently implement `IntoBytes`.
        unsafe {
            *decoded.as_mut_ptr() = value.as_mut_ptr();
        }
    }

    /// Sets the decoded value of the pointer to the first element of a slice.
    pub fn set_decoded_slice(slot: Slot<'_, Self>, mut slice: Slot<'de, [T]>) {
        // `unsafe` block required in the next version of munge
        #[allow(unused_unsafe)]
        // SAFETY: `slot` is a valid `Slot` of `Pointer`. Destructuring it is safe.
        let mut decoded = unsafe {
            munge!(let Self { decoded } = slot);
            decoded
        };
        // SAFETY: Identical to `decoded.write(ptr.into_raw())`, but raw
        // pointers don't currently implement `IntoBytes`.
        unsafe {
            *decoded.as_mut_ptr() = slice.as_mut_ptr().cast();
        }
    }

    /// Returns the underlying pointer.
    pub fn as_ptr(&self) -> *mut T {
        // SAFETY: Reading a raw pointer from a union is safe because raw pointers have no validity
        // invariants. The caller must ensure the pointer is valid before dereferencing it.
        unsafe { self.decoded }
    }
}
