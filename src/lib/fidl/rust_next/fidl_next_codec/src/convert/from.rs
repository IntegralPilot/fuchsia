// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::{MaybeUninit, forget};
use core::ptr::copy_nonoverlapping;

use crate::CopyOptimization;

/// A type which is convertible from an owned value.
pub trait FromWire<W>: Sized {
    /// Whether the conversion from `W` to `Self` is equivalent to copying the raw bytes of `W`.
    ///
    /// Copy optimization is disabled by default.
    const COPY_OPTIMIZATION: CopyOptimization<W, Self> = CopyOptimization::disable();

    /// Converts the given owned value to this type.
    fn from_wire(wire: W) -> Self;
}

/// A type which is convertible from a reference.
pub trait FromWireRef<W>: FromWire<W> {
    /// Converts the given reference to this type.
    fn from_wire_ref(wire: &W) -> Self;
}

/// An optional type which is convertible from an owned value.
pub trait FromWireOption<W>: Sized {
    /// Converts the given owned value to an option of this type.
    fn from_wire_option(wire: W) -> Option<Self>;
}

/// An optional type which is convertible from a reference.
pub trait FromWireOptionRef<W>: FromWireOption<W> {
    /// Converts the given reference to an option of this type.
    fn from_wire_option_ref(wire: &W) -> Option<Self>;
}

impl<T: FromWire<W>, W, const N: usize> FromWire<[W; N]> for [T; N] {
    fn from_wire(wire: [W; N]) -> Self {
        let mut result = MaybeUninit::<[T; N]>::uninit();
        if T::COPY_OPTIMIZATION.is_enabled() {
            // SAFETY: `T` has copy optimization enabled and so is safe to copy bytewise.
            unsafe {
                copy_nonoverlapping(wire.as_ptr().cast(), result.as_mut_ptr(), 1);
            }
            forget(wire);
        } else {
            for (i, item) in wire.into_iter().enumerate() {
                // SAFETY: `i` is in bounds for the array of size `N`, and the pointer is valid to
                // write.
                unsafe {
                    result.as_mut_ptr().cast::<T>().add(i).write(T::from_wire(item));
                }
            }
        }
        // SAFETY: All `N` elements of `result` have been initialized.
        unsafe { result.assume_init() }
    }
}

impl<T: FromWireRef<W>, W, const N: usize> FromWireRef<[W; N]> for [T; N] {
    fn from_wire_ref(wire: &[W; N]) -> Self {
        let mut result = MaybeUninit::<[T; N]>::uninit();
        if T::COPY_OPTIMIZATION.is_enabled() {
            // SAFETY: `T` has copy optimization enabled and so is safe to copy bytewise.
            unsafe {
                copy_nonoverlapping(wire.as_ptr().cast(), result.as_mut_ptr(), 1);
            }
        } else {
            for (i, item) in wire.iter().enumerate() {
                // SAFETY: `i` is in bounds for the array of size `N`, and the pointer is valid to
                // write.
                unsafe {
                    result.as_mut_ptr().cast::<T>().add(i).write(T::from_wire_ref(item));
                }
            }
        }
        // SAFETY: All `N` elements of `result` have been initialized.
        unsafe { result.assume_init() }
    }
}

impl<T: FromWire<W>, W> FromWire<W> for Box<T> {
    fn from_wire(wire: W) -> Self {
        Box::new(T::from_wire(wire))
    }
}

impl<T: FromWireRef<W>, W> FromWireRef<W> for Box<T> {
    fn from_wire_ref(wire: &W) -> Self {
        Box::new(T::from_wire_ref(wire))
    }
}

impl<T: FromWireOption<W>, W> FromWire<W> for Option<T> {
    fn from_wire(wire: W) -> Self {
        T::from_wire_option(wire)
    }
}

impl<T: FromWireOptionRef<W>, W> FromWireRef<W> for Option<T> {
    fn from_wire_ref(wire: &W) -> Self {
        T::from_wire_option_ref(wire)
    }
}
