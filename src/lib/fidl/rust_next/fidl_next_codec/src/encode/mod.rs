// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides encoding for FIDL types.

mod error;

use core::mem::MaybeUninit;
use core::ptr::copy_nonoverlapping;

pub use self::error::EncodeError;

use crate::{CopyOptimization, Wire};

/// Encodes a value.
///
/// # Safety
///
/// `encode` must initialize all non-padding bytes of `out`.
pub unsafe trait Encode<W: Wire, E: ?Sized>: Sized {
    /// Whether the conversion from `Self` to `W` is equivalent to copying the
    /// raw bytes of `Self`.
    ///
    /// Copy optimization is disabled by default.
    const COPY_OPTIMIZATION: CopyOptimization<Self, W> = CopyOptimization::disable();

    /// Encodes this value into an encoder and output.
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<W>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError>;
}

/// Encodes an optional value.
///
/// # Safety
///
/// `encode_option` must initialize all non-padding bytes of `out`.
pub unsafe trait EncodeOption<W: Wire, E: ?Sized>: Sized {
    /// Encodes this optional value into an encoder and output.
    fn encode_option(
        this: Option<Self>,
        encoder: &mut E,
        out: &mut MaybeUninit<W>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError>;
}

// SAFETY: Delegates to `T::encode` which guarantees that `out` is initialized.
unsafe impl<W, E, T> Encode<W, E> for Box<T>
where
    W: Wire,
    E: ?Sized,
    T: Encode<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<W>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        T::encode(*self, encoder, out, constraint)
    }
}

// SAFETY: Delegates to `<&'a T>::encode` which guarantees that `out` is initialized.
unsafe impl<'a, W, E, T> Encode<W, E> for &'a Box<T>
where
    W: Wire,
    E: ?Sized,
    &'a T: Encode<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<W>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        <&'a T>::encode(self, encoder, out, constraint)
    }
}

// SAFETY: Delegates to `T::encode_option` which guarantees that `out` is initialized.
unsafe impl<W, E, T> EncodeOption<W, E> for Box<T>
where
    W: Wire,
    E: ?Sized,
    T: EncodeOption<W, E>,
{
    fn encode_option(
        this: Option<Self>,
        encoder: &mut E,
        out: &mut MaybeUninit<W>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        T::encode_option(this.map(|value| *value), encoder, out, constraint)
    }
}

// SAFETY: Delegates to `<&'a T>::encode_option` which guarantees that `out` is initialized.
unsafe impl<'a, W, E, T> EncodeOption<W, E> for &'a Box<T>
where
    W: Wire,
    E: ?Sized,
    &'a T: EncodeOption<W, E>,
{
    fn encode_option(
        this: Option<Self>,
        encoder: &mut E,
        out: &mut MaybeUninit<W>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        <&'a T>::encode_option(this.map(|value| &**value), encoder, out, constraint)
    }
}

fn encode_to_array<A, W, E, T, const N: usize>(
    value: A,
    encoder: &mut E,
    out: &mut MaybeUninit<[W; N]>,
    constraint: W::Constraint,
) -> Result<(), EncodeError>
where
    A: AsRef<[T]> + IntoIterator,
    A::Item: Encode<W, E>,
    W: Wire,
    E: ?Sized,
    T: Encode<W, E>,
{
    if T::COPY_OPTIMIZATION.is_enabled() {
        // SAFETY: `T` has copy optimization enabled and so is safe to copy to the output.
        unsafe {
            copy_nonoverlapping(value.as_ref().as_ptr().cast(), out.as_mut_ptr(), 1);
        }
    } else {
        for (i, item) in value.into_iter().enumerate() {
            // SAFETY: `out` is a `MaybeUninit<[T::Encoded; N]>` and so consists of `N` copies of
            // `T::Encoded` in order with no additional padding. We can make a `&mut MaybeUninit` to
            // the `i`th element by:
            // 1. Getting a pointer to the contents of the `MaybeUninit<[T::Encoded; N]>` (the
            //    pointer is of type `*mut [T::Encoded; N]`).
            // 2. Casting it to `*mut MaybeUninit<T::Encoded>`. Note that `MaybeUninit<T>` always
            //    has the same layout as `T`.
            // 3. Adding `i` to reach the `i`th element.
            // 4. Dereferencing as `&mut`.
            let out_i = unsafe { &mut *out.as_mut_ptr().cast::<MaybeUninit<W>>().add(i) };
            item.encode(encoder, out_i, constraint)?;
        }
    }
    Ok(())
}

// SAFETY: `encode_to_array` initializes all elements of the array.
unsafe impl<W, E, T, const N: usize> Encode<[W; N], E> for [T; N]
where
    W: Wire,
    E: ?Sized,
    T: Encode<W, E>,
{
    const COPY_OPTIMIZATION: CopyOptimization<Self, [W; N]> = T::COPY_OPTIMIZATION.infer_array();

    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<[W; N]>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        encode_to_array(self, encoder, out, constraint)
    }
}

// SAFETY: `encode_to_array` initializes all elements of the array.
unsafe impl<'a, W, E, T, const N: usize> Encode<[W; N], E> for &'a [T; N]
where
    W: Wire,
    E: ?Sized,
    T: Encode<W, E>,
    &'a T: Encode<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<[W; N]>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        encode_to_array(self, encoder, out, constraint)
    }
}

// SAFETY: Delegates to `T::encode_option` which guarantees that `out` is initialized.
unsafe impl<W, E, T> Encode<W, E> for Option<T>
where
    W: Wire,
    E: ?Sized,
    T: EncodeOption<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<W>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        T::encode_option(self, encoder, out, constraint)
    }
}

// SAFETY: Delegates to `<Option<&'a T>>::encode` which guarantees that `out` is initialized.
unsafe impl<'a, W, E, T> Encode<W, E> for &'a Option<T>
where
    W: Wire,
    E: ?Sized,
    Option<&'a T>: Encode<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<W>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        self.as_ref().encode(encoder, out, constraint)
    }
}
