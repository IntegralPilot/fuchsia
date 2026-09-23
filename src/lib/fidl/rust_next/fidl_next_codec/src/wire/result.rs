// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;
use core::marker::PhantomData;
use core::mem::{ManuallyDrop, MaybeUninit};

use crate::{
    Chunk, Constrained, Decode, DecodeError, Decoder, Encode, EncodeError, Encoder, FromWire,
    FromWireRef, IntoNatural, Slot, ValidationError, Wire, munge, wire,
};

use core::result::Result as CoreResult;

/// A FIDL result union.
#[repr(transparent)]
pub struct Result<'de, T, E> {
    raw: wire::Union,
    _phantom: PhantomData<(&'de mut [Chunk], T, E)>,
}

impl<T, E> Drop for Result<'_, T, E> {
    fn drop(&mut self) {
        match self.raw.ordinal() {
            ORD_OK => {
                // SAFETY: The ordinal is `ORD_OK`, so the union contains a valid initialized `T`.
                // We read it to drop it.
                let _ = unsafe { self.raw.get().read_unchecked::<T>() };
            }
            ORD_ERR => {
                // SAFETY: The ordinal is `ORD_ERR`, so the union contains a valid initialized `E`.
                // We read it to drop it.
                let _ = unsafe { self.raw.get().read_unchecked::<E>() };
            }
            // SAFETY: The ordinal of a validated `Result` must be either `ORD_OK` or `ORD_ERR`.
            _ => unsafe { ::core::hint::unreachable_unchecked() },
        }
    }
}

impl<T, E> Constrained for Result<'_, T, E>
where
    T: Constrained<Constraint = ()>,
    E: Constrained<Constraint = ()>,
{
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> CoreResult<(), ValidationError> {
        Ok(())
    }
}

// SAFETY: `Result` is a `#[repr(transparent)]` wrapper around `wire::Union`, which is `Wire`.
// The generic parameters `T` and `E` are also `Wire`.
unsafe impl<T, E> Wire for Result<'static, T, E>
where
    T: Wire<Constraint = ()>,
    E: Wire<Constraint = ()>,
{
    type Narrowed<'de> = Result<'de, T::Narrowed<'de>, E::Narrowed<'de>>;

    #[inline]
    fn zero_padding(out: &mut MaybeUninit<Self>) {
        munge!(let Self { raw, _phantom: _ } = out);
        wire::Union::zero_padding(raw);
    }
}

const ORD_OK: u64 = 1;
const ORD_ERR: u64 = 2;

impl<T, E> Result<'_, T, E> {
    /// Returns whether the result is `Ok`.
    pub fn is_ok(&self) -> bool {
        self.raw.ordinal() == ORD_OK
    }

    /// Returns whether the result is `Err`.
    pub fn is_err(&self) -> bool {
        self.raw.ordinal() == ORD_ERR
    }

    /// Returns the `Ok` value of the result, if any.
    pub fn ok(&self) -> Option<&T> {
        // SAFETY: The ordinal is `ORD_OK`, so the union contains a valid initialized `T`.
        self.is_ok().then(|| unsafe { self.raw.get().deref_unchecked() })
    }

    /// Returns the `Err` value of the result, if any.
    pub fn err(&self) -> Option<&E> {
        // SAFETY: The ordinal is `ORD_ERR`, so the union contains a valid initialized `E`.
        self.is_err().then(|| unsafe { self.raw.get().deref_unchecked() })
    }

    /// Returns the contained `Ok` value.
    ///
    /// Panics if the result was not `Ok`.
    pub fn unwrap(&self) -> &T {
        self.ok().unwrap()
    }

    /// Returns the contained `Err` value.
    ///
    /// Panics if the result was not `Err`.
    pub fn unwrap_err(&self) -> &E {
        self.err().unwrap()
    }

    /// Returns a `Result` of a reference to the value or error.
    pub fn as_ref(&self) -> CoreResult<&T, &E> {
        match self.raw.ordinal() {
            // SAFETY: The ordinal is `ORD_OK`, so the union contains a valid initialized `T`.
            ORD_OK => unsafe { Ok(self.raw.get().deref_unchecked()) },
            // SAFETY: The ordinal is `ORD_ERR`, so the union contains a valid initialized `E`.
            ORD_ERR => unsafe { Err(self.raw.get().deref_unchecked()) },
            // SAFETY: The ordinal of a validated `Result` must be either `ORD_OK` or `ORD_ERR`.
            _ => unsafe { ::core::hint::unreachable_unchecked() },
        }
    }

    /// Returns a `Result` of the owned value or error.
    pub fn into_result(self) -> CoreResult<T, E> {
        let this = ManuallyDrop::new(self);
        match this.raw.ordinal() {
            // SAFETY: The ordinal is `ORD_OK`, so the union contains a valid initialized `T`.
            // We use `ManuallyDrop` to prevent double-dropping.
            ORD_OK => unsafe { Ok(this.raw.get().read_unchecked()) },
            // SAFETY: The ordinal is `ORD_ERR`, so the union contains a valid initialized `E`.
            // We use `ManuallyDrop` to prevent double-dropping.
            ORD_ERR => unsafe { Err(this.raw.get().read_unchecked()) },
            // SAFETY: The ordinal of a validated `Result` must be either `ORD_OK` or `ORD_ERR`.
            _ => unsafe { ::core::hint::unreachable_unchecked() },
        }
    }
}

impl<T: Clone, E: Clone> Clone for Result<'_, T, E> {
    fn clone(&self) -> Self {
        Self {
            raw: match self.raw.ordinal() {
                // SAFETY: The ordinal is `ORD_OK`, so the union contains a valid initialized `T`.
                ORD_OK => unsafe { self.raw.clone_inline_unchecked::<T>() },
                // SAFETY: The ordinal is `ORD_ERR`, so the union contains a valid initialized `E`.
                ORD_ERR => unsafe { self.raw.clone_inline_unchecked::<E>() },
                // SAFETY: The ordinal of a validated `Result` must be either `ORD_OK` or
                // `ORD_ERR`.
                _ => unsafe { ::core::hint::unreachable_unchecked() },
            },
            _phantom: PhantomData,
        }
    }
}

impl<T, E> fmt::Debug for Result<'_, T, E>
where
    T: fmt::Debug,
    E: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

// SAFETY: If `decode` returns `Ok`, `slot` is guaranteed to contain a valid decoded `Result`
// because it delegates to `wire::Union::decode_as` which validates and decodes the active variant.
unsafe impl<'de, D, T, E> Decode<D> for Result<'de, T, E>
where
    D: Decoder<'de> + ?Sized,
    T: Decode<D, Constraint = ()>,
    E: Decode<D, Constraint = ()>,
{
    fn decode(slot: Slot<'_, Self>, decoder: &mut D, _: ()) -> CoreResult<(), DecodeError> {
        munge!(let Self { mut raw, _phantom: _ } = slot);

        match wire::Union::encoded_ordinal(raw.as_mut()) {
            ORD_OK => wire::Union::decode_as::<D, T>(raw, decoder, ())?,
            ORD_ERR => wire::Union::decode_as::<D, E>(raw, decoder, ())?,
            ord => return Err(DecodeError::InvalidUnionOrdinal(ord as usize)),
        }

        Ok(())
    }
}

// SAFETY: `Result` is `#[repr(transparent)]` over `wire::Union`. `encode` delegates to
// `wire::Union::encode_as`, which fully initializes the underlying `Union`, thus initializing
// `Result`.
unsafe impl<Enc, WT, T, WE, E> Encode<Result<'static, WT, WE>, Enc> for CoreResult<T, E>
where
    Enc: Encoder + ?Sized,
    WT: Wire<Constraint = ()>,
    T: Encode<WT, Enc>,
    WE: Wire<Constraint = ()>,
    E: Encode<WE, Enc>,
{
    fn encode(
        self,
        encoder: &mut Enc,
        out: &mut MaybeUninit<Result<'static, WT, WE>>,
        _: (),
    ) -> CoreResult<(), EncodeError> {
        munge!(let Result { raw, _phantom: _ } = out);

        match self {
            Ok(value) => wire::Union::encode_as::<Enc, WT>(value, ORD_OK, encoder, raw, ())?,
            Err(error) => wire::Union::encode_as::<Enc, WE>(error, ORD_ERR, encoder, raw, ())?,
        }

        Ok(())
    }
}

// SAFETY: Delegates to the `Encode` implementation for `CoreResult`, which is safe.
unsafe impl<'a, Enc, WT, T, WE, E> Encode<Result<'static, WT, WE>, Enc> for &'a CoreResult<T, E>
where
    Enc: Encoder + ?Sized,
    WT: Wire<Constraint = ()>,
    &'a T: Encode<WT, Enc>,
    WE: Wire<Constraint = ()>,
    &'a E: Encode<WE, Enc>,
{
    fn encode(
        self,
        encoder: &mut Enc,
        out: &mut MaybeUninit<Result<'static, WT, WE>>,
        _: (),
    ) -> CoreResult<(), EncodeError> {
        self.as_ref().encode(encoder, out, ())
    }
}

impl<T, E, WT, WE> FromWire<Result<'_, WT, WE>> for CoreResult<T, E>
where
    T: FromWire<WT>,
    E: FromWire<WE>,
{
    #[inline]
    fn from_wire(wire: Result<'_, WT, WE>) -> Self {
        match wire.into_result() {
            Ok(value) => Ok(T::from_wire(value)),
            Err(error) => Err(E::from_wire(error)),
        }
    }
}

impl<T: IntoNatural, E: IntoNatural> IntoNatural for Result<'_, T, E> {
    type Natural = CoreResult<T::Natural, E::Natural>;
}

impl<T, E, WT, WE> FromWireRef<Result<'_, WT, WE>> for CoreResult<T, E>
where
    T: FromWireRef<WT>,
    E: FromWireRef<WE>,
{
    #[inline]
    fn from_wire_ref(wire: &Result<'_, WT, WE>) -> Self {
        match wire.as_ref() {
            Ok(value) => Ok(T::from_wire_ref(value)),
            Err(error) => Err(E::from_wire_ref(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{DecoderExt as _, EncoderExt as _, chunks, wire};

    #[test]
    fn encode_result() {
        assert_eq!(
            Vec::encode(Result::<i32, i32>::Ok(0x12345678)).unwrap(),
            chunks![
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x78, 0x56, 0x34, 0x12, 0x00, 0x00,
                0x01, 0x00,
            ],
        );
        assert_eq!(
            Vec::encode(Result::<i32, i32>::Err(0x12345678)).unwrap(),
            chunks![
                0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x78, 0x56, 0x34, 0x12, 0x00, 0x00,
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
            .decode::<wire::Result<'_, wire::Int32, wire::Int32>>()
            .unwrap()
            .as_ref()
            .unwrap()
            .0,
            0x12345678,
        );
        assert_eq!(
            chunks![
                0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x78, 0x56, 0x34, 0x12, 0x00, 0x00,
                0x01, 0x00,
            ]
            .as_mut_slice()
            .decode::<wire::Result<'_, wire::Int32, wire::Int32>>()
            .unwrap()
            .as_ref()
            .unwrap_err()
            .0,
            0x12345678,
        );
    }
}
