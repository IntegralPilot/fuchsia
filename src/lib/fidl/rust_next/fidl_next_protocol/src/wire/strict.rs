// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::MaybeUninit;
use core::ops::Deref;

use fidl_next_codec::{
    Constrained, Decode, DecodeError, Encode, EncodeError, Encoder, FromWire, FromWireRef,
    IntoNatural, Slot, ValidationError, Wire, munge,
};

/// A strict FIDL message.
#[derive(Clone, Debug)]
#[repr(transparent)]
pub struct Strict<T> {
    inner: T,
}

impl<T> Constrained for Strict<T>
where
    T: Constrained,
{
    type Constraint = T::Constraint;

    fn validate(slot: Slot<'_, Self>, constraint: Self::Constraint) -> Result<(), ValidationError> {
        munge!(let Self { inner } = slot);
        T::validate(inner, constraint)
    }
}

unsafe impl<T> Wire for Strict<T>
where
    T: Wire,
{
    type Narrowed<'de> = Strict<T::Narrowed<'de>>;

    #[inline]
    fn zero_padding(out: &mut MaybeUninit<Self>) {
        munge!(let Self { inner } = out);
        T::zero_padding(inner);
    }
}

impl<T> Deref for Strict<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> AsRef<T> for Strict<T> {
    fn as_ref(&self) -> &T {
        Deref::deref(self)
    }
}

impl<T> Strict<T> {
    /// Consumes the `Strict`, returning the contained value.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

unsafe impl<D, T> Decode<D> for Strict<T>
where
    D: ?Sized,
    T: Decode<D>,
{
    fn decode(
        slot: Slot<'_, Self>,
        decoder: &mut D,
        constraint: T::Constraint,
    ) -> Result<(), DecodeError> {
        munge!(let Self { inner } = slot);

        T::decode(inner, decoder, constraint)
    }
}

unsafe impl<E, W, T> Encode<Strict<W>, E> for crate::Strict<T>
where
    E: Encoder + ?Sized,
    W: Wire,
    T: Encode<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<Strict<W>>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        munge!(let Strict { inner } = out);
        T::encode(self.0, encoder, inner, constraint)?;
        Ok(())
    }
}

unsafe impl<'a, E, W, T> Encode<Strict<W>, E> for &'a crate::Strict<T>
where
    E: Encoder + ?Sized,
    W: Wire,
    &'a T: Encode<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<Strict<W>>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        crate::Strict(self.as_ref()).encode(encoder, out, constraint)
    }
}

impl<T, W> FromWire<Strict<W>> for crate::Strict<T>
where
    T: FromWire<W>,
{
    #[inline]
    fn from_wire(wire: Strict<W>) -> Self {
        crate::Strict(T::from_wire(wire.inner))
    }
}

impl<T: IntoNatural> IntoNatural for Strict<T> {
    type Natural = crate::Strict<T::Natural>;
}

impl<T, W> FromWireRef<Strict<W>> for crate::Strict<T>
where
    T: FromWireRef<W>,
{
    #[inline]
    fn from_wire_ref(wire: &Strict<W>) -> Self {
        crate::Strict(T::from_wire_ref(wire.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::Strict;

    use fidl_next_codec::{DecoderExt as _, EncoderExt as _, chunks, wire};

    #[test]
    fn encode_strict() {
        assert_eq!(
            Vec::encode(crate::Strict::<i32>(0x12345678)).unwrap(),
            chunks![0x78, 0x56, 0x34, 0x12, 0x00, 0x00, 0x00, 0x00],
        );
    }

    #[test]
    fn decode_strict() {
        assert_eq!(
            chunks![0x78, 0x56, 0x34, 0x12, 0x00, 0x00, 0x00, 0x00]
                .as_mut_slice()
                .decode::<Strict<wire::Int32>>()
                .unwrap()
                .as_ref()
                .0,
            0x12345678,
        );
    }
}
