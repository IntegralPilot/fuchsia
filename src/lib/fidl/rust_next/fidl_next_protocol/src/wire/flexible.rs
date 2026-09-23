// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;
use core::marker::PhantomData;
use core::mem::{MaybeUninit, forget};
use core::ops::Deref;

use fidl_next_codec::{
    Chunk, Constrained, Decode, DecodeError, Decoder, Encode, EncodeError, Encoder, FromWire,
    FromWireRef, IntoNatural, Slot, ValidationError, Wire, munge, wire,
};

const ORD_OK: u64 = 1;

/// A flexible FIDL result union.
#[repr(transparent)]
pub struct Flexible<'de, T> {
    raw: wire::Union,
    _phantom: PhantomData<(&'de mut [Chunk], T)>,
}

impl<T> Drop for Flexible<'_, T> {
    fn drop(&mut self) {
        let _ = unsafe { self.raw.get().read_unchecked::<T>() };
    }
}

impl<T> Constrained for Flexible<'_, T>
where
    T: Constrained<Constraint = ()>,
{
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

unsafe impl<T> Wire for Flexible<'static, T>
where
    T: Wire<Constraint = ()>,
{
    type Narrowed<'de> = Flexible<'de, T::Narrowed<'de>>;

    #[inline]
    fn zero_padding(out: &mut MaybeUninit<Self>) {
        munge!(let Self { raw, _phantom: _ } = out);
        wire::Union::zero_padding(raw);
    }
}

impl<T> Deref for Flexible<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.raw.get().deref_unchecked() }
    }
}

impl<T> AsRef<T> for Flexible<'_, T> {
    fn as_ref(&self) -> &T {
        Deref::deref(self)
    }
}

impl<T> Flexible<'_, T> {
    /// Consumes the `Flexible`, returning the contained value.
    pub fn into_inner(self) -> T {
        let result = unsafe { self.raw.get().read_unchecked() };
        forget(self);
        result
    }
}

impl<T: Clone> Clone for Flexible<'_, T> {
    fn clone(&self) -> Self {
        Self { raw: unsafe { self.raw.clone_inline_unchecked::<T>() }, _phantom: PhantomData }
    }
}

impl<T: fmt::Debug> fmt::Debug for Flexible<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

unsafe impl<'de, D, T> Decode<D> for Flexible<'de, T>
where
    D: Decoder<'de> + ?Sized,
    T: Decode<D, Constraint = ()>,
{
    fn decode(slot: Slot<'_, Self>, decoder: &mut D, _: ()) -> Result<(), DecodeError> {
        munge!(let Self { mut raw, _phantom: _ } = slot);

        let ordinal = wire::Union::encoded_ordinal(raw.as_mut());
        if ordinal != ORD_OK {
            return Err(DecodeError::InvalidUnionOrdinal(ordinal as usize));
        }
        wire::Union::decode_as::<D, T>(raw, decoder, ())?;
        Ok(())
    }
}

unsafe impl<E, W, T> Encode<Flexible<'static, W>, E> for crate::Flexible<T>
where
    E: Encoder + ?Sized,
    W: Wire<Constraint = ()>,
    T: Encode<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<Flexible<'static, W>>,
        _: (),
    ) -> Result<(), EncodeError> {
        munge!(let Flexible { raw, _phantom: _ } = out);
        wire::Union::encode_as::<E, W>(self.0, ORD_OK, encoder, raw, ())?;
        Ok(())
    }
}

unsafe impl<'a, E, W, T> Encode<Flexible<'static, W>, E> for &'a crate::Flexible<T>
where
    E: Encoder + ?Sized,
    W: Wire<Constraint = ()>,
    &'a T: Encode<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<Flexible<'static, W>>,
        _: (),
    ) -> Result<(), EncodeError> {
        crate::Flexible(self.as_ref()).encode(encoder, out, ())
    }
}

impl<T, W> FromWire<Flexible<'_, W>> for crate::Flexible<T>
where
    T: FromWire<W>,
{
    #[inline]
    fn from_wire(wire: Flexible<'_, W>) -> Self {
        crate::Flexible(T::from_wire(wire.into_inner()))
    }
}

impl<T: IntoNatural> IntoNatural for Flexible<'_, T> {
    type Natural = crate::Flexible<T::Natural>;
}

impl<T, W> FromWireRef<Flexible<'_, W>> for crate::Flexible<T>
where
    T: FromWireRef<W>,
{
    #[inline]
    fn from_wire_ref(wire: &Flexible<'_, W>) -> Self {
        crate::Flexible(T::from_wire_ref(wire.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::Flexible;

    use fidl_next_codec::{DecoderExt as _, EncoderExt as _, chunks, wire};

    #[test]
    fn encode_flexible() {
        assert_eq!(
            Vec::encode(crate::Flexible::<i32>(0x12345678)).unwrap(),
            chunks![
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x78, 0x56, 0x34, 0x12, 0x00, 0x00,
                0x01, 0x00,
            ],
        );
    }

    #[test]
    fn decode_result() {
        assert_eq!(
            chunks![
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x78, 0x56, 0x34, 0x12, 0x00, 0x00,
                0x01, 0x00,
            ]
            .as_mut_slice()
            .decode::<Flexible<'_, wire::Int32>>()
            .unwrap()
            .as_ref()
            .0,
            0x12345678,
        );
    }
}
