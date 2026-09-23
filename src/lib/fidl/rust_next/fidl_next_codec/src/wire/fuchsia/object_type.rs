// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;
use core::mem::MaybeUninit;

use munge::munge;

use crate::{
    Constrained, Decode, DecodeError, Encode, EncodeError, FromWire, FromWireRef, IntoNatural,
    Slot, ValidationError, Wire, wire,
};

/// The wire type for [`zx::ObjectType`].
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct ObjectType {
    inner: wire::Uint32,
}

impl Constrained for ObjectType {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

// SAFETY: `ObjectType` is a `#[repr(transparent)]` wrapper around `wire::Uint32`, which is `Wire`.
unsafe impl Wire for ObjectType {
    type Narrowed<'de> = Self;

    #[inline]
    fn zero_padding(out: &mut MaybeUninit<Self>) {
        munge!(let Self { inner } = out);
        wire::Uint32::zero_padding(inner);
    }
}

impl ObjectType {
    /// Returns an `ObjectType` with the same value as this wire type.
    pub fn to_object_type(self) -> zx::ObjectType {
        zx::ObjectType::from_raw(*self.inner)
    }
}

impl From<zx::ObjectType> for ObjectType {
    fn from(value: zx::ObjectType) -> Self {
        Self { inner: wire::Uint32(value.into_raw()) }
    }
}

impl fmt::Debug for ObjectType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.to_object_type().fmt(f)
    }
}

// SAFETY: If `decode` returns `Ok`, `slot` is guaranteed to contain a valid decoded `ObjectType`
// because it delegates to `wire::Uint32::decode` which guarantees the slot is valid.
unsafe impl<D: ?Sized> Decode<D> for ObjectType {
    fn decode(
        slot: Slot<'_, Self>,
        decoder: &mut D,
        _: Self::Constraint,
    ) -> Result<(), DecodeError> {
        munge!(let Self { inner } = slot);
        wire::Uint32::decode(inner, decoder, ())
    }
}

// SAFETY: `ObjectType` is `#[repr(transparent)]` over `wire::Uint32`. `encode` delegates to
// the `Encode` implementation for `u32` (via `into_raw()`), which fully initializes the
// underlying `Uint32`, thus initializing `ObjectType`.
unsafe impl<E: ?Sized> Encode<ObjectType, E> for zx::ObjectType {
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<ObjectType>,
        constraint: (),
    ) -> Result<(), EncodeError> {
        munge!(let ObjectType { inner } = out);
        self.into_raw().encode(encoder, inner, constraint)
    }
}

// SAFETY: Delegates to the `Encode` implementation for `zx::ObjectType`, which is safe.
unsafe impl<E: ?Sized> Encode<ObjectType, E> for &zx::ObjectType {
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<ObjectType>,
        constraint: (),
    ) -> Result<(), EncodeError> {
        Encode::encode(*self, encoder, out, constraint)
    }
}

impl FromWire<ObjectType> for zx::ObjectType {
    fn from_wire(wire: ObjectType) -> Self {
        Self::from_wire_ref(&wire)
    }
}

impl FromWireRef<ObjectType> for zx::ObjectType {
    fn from_wire_ref(wire: &ObjectType) -> Self {
        Self::from_raw(*wire.inner)
    }
}

impl IntoNatural for ObjectType {
    type Natural = zx::ObjectType;
}
