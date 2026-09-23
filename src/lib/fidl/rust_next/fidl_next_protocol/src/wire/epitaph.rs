// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::MaybeUninit;

use fidl_next_codec::{
    Constrained, Decode, DecodeError, Encode, EncodeError, Slot, ValidationError, Wire,
};
use zerocopy::IntoBytes;

use crate::wire;

/// A FIDL protocol epitaph.
#[derive(Clone, Copy, Debug, IntoBytes)]
#[repr(C)]
pub struct Epitaph {
    /// The error status.
    pub error: wire::Int32,
}

impl Epitaph {
    /// Returns a new epitaph with the given error.
    pub fn new(error: i32) -> Self {
        Self { error: wire::Int32(error) }
    }
}

impl Constrained for Epitaph {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

unsafe impl Wire for Epitaph {
    type Narrowed<'de> = Self;

    #[inline]
    fn zero_padding(_: &mut MaybeUninit<Self>) {
        // Wire epitaphs have no padding
    }
}

unsafe impl<E: ?Sized> Encode<Epitaph, E> for Epitaph {
    #[inline]
    fn encode(self, _: &mut E, out: &mut MaybeUninit<Epitaph>, _: ()) -> Result<(), EncodeError> {
        out.write(self);
        Ok(())
    }
}

unsafe impl<E: ?Sized> Encode<Epitaph, E> for &Epitaph {
    #[inline]
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<Epitaph>,
        constraint: (),
    ) -> Result<(), EncodeError> {
        Encode::encode(*self, encoder, out, constraint)
    }
}

unsafe impl<D: ?Sized> Decode<D> for Epitaph {
    #[inline]
    fn decode(_: Slot<'_, Self>, _: &mut D, _: ()) -> Result<(), DecodeError> {
        Ok(())
    }
}
