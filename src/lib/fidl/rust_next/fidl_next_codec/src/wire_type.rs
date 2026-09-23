// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::MaybeUninit;

use crate::Constrained;

/// A FIDL wire type.
///
/// # Safety
///
/// ## Lifetime erasure
///
/// Wire types allow for lifetime erasure and restoration. A type that
/// implements `Wire` may have an instance of its [`Narrowed`][Wire::Narrowed]
/// type erased by transmuting to an instance of itself.
///
/// It is only safe to expose values and mutable reference to the narrowed
/// versions of wire types. While type erased, it is only safe to expose shared
/// references to the implementing type (i.e. `&'de Foo<'static>`).
///
/// ## Padding
///
/// `zero_padding` must write zeroes to (at least) the padding bytes of `out`.
pub unsafe trait Wire: 'static + Sized + Constrained {
    /// The narrowed wire type, restricted to the `'de` lifetime.
    type Narrowed<'de>: Constrained<Constraint = Self::Constraint>;

    /// Writes zeroes to the padding for this type, if any.
    fn zero_padding(out: &mut MaybeUninit<Self>);
}

// SAFETY: Slices of `Wire` types are also valid `Wire` types because elements are laid out
// sequentially with no padding between them, and lifetime erasure is safe if it is safe for `T`.
unsafe impl<T: Wire, const N: usize> Wire for [T; N] {
    type Narrowed<'de> = [T::Narrowed<'de>; N];

    #[inline]
    fn zero_padding(out: &mut MaybeUninit<Self>) {
        for i in 0..N {
            // SAFETY: `i` is in bounds for the array of size `N`, and the cast to `MaybeUninit<T>`
            // is valid because `MaybeUninit<T>` has the same layout as `T`.
            let out_i = unsafe { &mut *out.as_mut_ptr().cast::<MaybeUninit<T>>().add(i) };
            T::zero_padding(out_i);
        }
    }
}
