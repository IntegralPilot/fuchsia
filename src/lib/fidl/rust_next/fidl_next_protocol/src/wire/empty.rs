// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::MaybeUninit;

use fidl_next_codec::{
    Constrained, Decode, Encode, EncodeError, FromWire, FromWireRef, IntoNatural, Slot,
    ValidationError, Wire,
};

/// The wire type for an empty message body.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct EmptyMessageBody;

impl Constrained for EmptyMessageBody {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

unsafe impl Wire for EmptyMessageBody {
    type Narrowed<'de> = Self;

    #[inline]
    fn zero_padding(_: &mut MaybeUninit<Self>) {
        // Empty message bodies have no padding
    }
}

unsafe impl<E: ?Sized> Encode<EmptyMessageBody, E> for () {
    #[inline]
    fn encode(
        self,
        _: &mut E,
        _: &mut MaybeUninit<EmptyMessageBody>,
        _: (),
    ) -> Result<(), EncodeError> {
        Ok(())
    }
}

unsafe impl<E: ?Sized> Encode<EmptyMessageBody, E> for &() {
    #[inline]
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<EmptyMessageBody>,
        _: (),
    ) -> Result<(), EncodeError> {
        Encode::encode((), encoder, out, ())
    }
}

unsafe impl<D: ?Sized> Decode<D> for EmptyMessageBody {
    #[inline]
    fn decode(
        _: Slot<'_, Self>,
        _: &mut D,
        _: Self::Constraint,
    ) -> Result<(), fidl_next_codec::DecodeError> {
        Ok(())
    }
}

impl FromWire<EmptyMessageBody> for () {
    fn from_wire(_: EmptyMessageBody) -> Self {}
}

impl FromWireRef<EmptyMessageBody> for () {
    fn from_wire_ref(_: &EmptyMessageBody) -> Self {}
}

impl IntoNatural for EmptyMessageBody {
    type Natural = ();
}
