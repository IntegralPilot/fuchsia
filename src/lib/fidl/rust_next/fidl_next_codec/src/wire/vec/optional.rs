// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::{MaybeUninit, needs_drop};
use core::{fmt, slice};

use munge::munge;

use super::raw::RawVector;
use crate::{
    Constrained, Decode, DecodeError, Decoder, DecoderExt as _, Encode, EncodeError, EncodeOption,
    Encoder, EncoderExt as _, FromWire, FromWireOption, FromWireOptionRef, FromWireRef,
    IntoNatural, Slot, ValidationError, Wire, wire,
};

/// An optional FIDL vector
#[repr(transparent)]
pub struct OptionalVector<'de, T> {
    raw: RawVector<'de, T>,
}

// SAFETY: `OptionalVector` is `repr(transparent)` over `RawVector`, which implements `Wire`.
// Lifetime erasure is safe since `OptionalVector` is covariant over its lifetime.
unsafe impl<T: Wire> Wire for OptionalVector<'static, T> {
    type Narrowed<'de> = OptionalVector<'de, T::Narrowed<'de>>;

    #[inline]
    fn zero_padding(out: &mut MaybeUninit<Self>) {
        munge!(let Self { raw } = out);
        RawVector::<T>::zero_padding(raw);
    }
}

impl<T> Drop for OptionalVector<'_, T> {
    fn drop(&mut self) {
        if needs_drop::<T>() && self.is_some() {
            // SAFETY: If the vector is present and `T` needs to be dropped, the pointer has
            // been decoded and points to a valid slice of initialized `T` elements.
            unsafe {
                self.raw.as_slice_ptr().drop_in_place();
            }
        }
    }
}

impl<'de, T> OptionalVector<'de, T> {
    /// Encodes that a vector is present in a slot.
    pub fn encode_present(out: &mut MaybeUninit<Self>, len: u64) {
        munge!(let Self { raw } = out);
        RawVector::encode_present(raw, len);
    }

    /// Encodes that a vector is absent in a slot.
    pub fn encode_absent(out: &mut MaybeUninit<Self>) {
        munge!(let Self { raw } = out);
        RawVector::encode_absent(raw);
    }

    /// Returns whether the vector is present.
    pub fn is_some(&self) -> bool {
        !self.raw.as_ptr().is_null()
    }

    /// Returns whether the vector is absent.
    pub fn is_none(&self) -> bool {
        !self.is_some()
    }

    /// Gets a reference to the vector, if any.
    pub fn as_ref(&self) -> Option<&wire::Vector<'_, T>> {
        if self.is_some() {
            // SAFETY: `OptionalVector` and `Vector` have the same layout (`repr(transparent)`
            // over `RawVector`). Since `self.is_some()` is true, the underlying pointer is
            // non-null, which satisfies the invariant of `Vector`.
            Some(unsafe { &*(self as *const Self).cast() })
        } else {
            None
        }
    }

    /// Converts the optional wire vector to an `Option<WireVector>`.
    pub fn to_option(self) -> Option<wire::Vector<'de, T>> {
        if self.is_some() {
            // SAFETY: `OptionalVector` and `Vector` have the same layout. Since `self.is_some()`
            // is true, the underlying pointer is non-null, which satisfies the invariant of
            // `Vector`.
            Some(unsafe { core::mem::transmute::<Self, wire::Vector<'de, T>>(self) })
        } else {
            None
        }
    }

    /// Decodes a wire vector which contains raw data.
    ///
    /// # Safety
    ///
    /// The elements of the wire vector must not need to be individually decoded, and must always be
    /// valid.
    pub unsafe fn decode_raw<D>(
        mut slot: Slot<'_, Self>,
        decoder: &mut D,
        max_len: u64,
    ) -> Result<(), DecodeError>
    where
        D: Decoder<'de> + ?Sized,
        T: Decode<D>,
    {
        munge!(let Self { raw: RawVector { len, mut ptr } } = slot.as_mut());

        if wire::Pointer::is_encoded_present(ptr.as_mut())? {
            if **len > max_len {
                return Err(DecodeError::Validation(ValidationError::VectorTooLong {
                    count: **len,
                    limit: max_len,
                }));
            }

            let slice = decoder.take_slice_slot::<T>(**len as usize)?;
            wire::Pointer::set_decoded_slice(ptr, slice);
        } else if *len != 0 {
            return Err(DecodeError::InvalidOptionalSize(**len));
        }

        Ok(())
    }

    /// Validate that this vector's length falls within the limit.
    pub(crate) fn validate_max_len(
        slot: Slot<'_, Self>,
        limit: u64,
    ) -> Result<(), ValidationError> {
        munge!(let Self { raw: RawVector { len, ptr } } = slot);
        let count = **len;
        let is_present = ptr.as_bytes() != [0; 8];
        if is_present && count > limit {
            Err(ValidationError::VectorTooLong { count, limit })
        } else {
            Ok(())
        }
    }
}

type VectorConstraint<T> = (u64, <T as Constrained>::Constraint);

impl<T: Constrained> Constrained for OptionalVector<'_, T> {
    type Constraint = VectorConstraint<T>;

    fn validate(slot: Slot<'_, Self>, constraint: Self::Constraint) -> Result<(), ValidationError> {
        let (limit, _member_constraint) = constraint;

        Self::validate_max_len(slot, limit)
    }
}

impl<T: fmt::Debug> fmt::Debug for OptionalVector<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_ref().fmt(f)
    }
}

impl<T, U> PartialEq<Option<U>> for OptionalVector<'_, T>
where
    for<'de> wire::Vector<'de, T>: PartialEq<U>,
{
    fn eq(&self, other: &Option<U>) -> bool {
        match (self.as_ref(), other.as_ref()) {
            (Some(lhs), Some(rhs)) => lhs == rhs,
            (None, None) => true,
            _ => false,
        }
    }
}

// SAFETY: If `decode` returns `Ok`, the `OptionalVector` has been successfully decoded.
// If present, the pointer is updated to point to a successfully decoded slice of `T`
// allocated by the decoder. If absent, the pointer remains null and the length is 0.
unsafe impl<'de, D, T> Decode<D> for OptionalVector<'de, T>
where
    D: Decoder<'de> + ?Sized,
    T: Decode<D>,
{
    fn decode(
        mut slot: Slot<'_, Self>,
        decoder: &mut D,
        constraint: Self::Constraint,
    ) -> Result<(), DecodeError> {
        munge!(let Self { raw: RawVector { len, mut ptr } } = slot.as_mut());

        let (length_constraint, member_constraint) = constraint;

        if wire::Pointer::is_encoded_present(ptr.as_mut())? {
            if **len > length_constraint {
                return Err(DecodeError::Validation(ValidationError::VectorTooLong {
                    count: **len,
                    limit: length_constraint,
                }));
            }

            let mut slice = decoder.take_slice_slot::<T>(**len as usize)?;
            for i in 0..**len as usize {
                T::decode(slice.index(i), decoder, member_constraint)?;
            }
            wire::Pointer::set_decoded_slice(ptr, slice);
        } else if *len != 0 {
            return Err(DecodeError::InvalidOptionalSize(**len));
        }

        Ok(())
    }
}

#[inline]
fn encode_to_optional_vector<V, W, E, T>(
    value: Option<V>,
    encoder: &mut E,
    out: &mut MaybeUninit<OptionalVector<'static, W>>,
    constraint: VectorConstraint<W>,
) -> Result<(), EncodeError>
where
    V: AsRef<[T]> + IntoIterator,
    V::IntoIter: ExactSizeIterator,
    V::Item: Encode<W, E>,
    W: Wire,
    E: Encoder + ?Sized,
    T: Encode<W, E>,
{
    let (length_constraint, member_constraint) = constraint;

    if let Some(value) = value {
        let len = value.as_ref().len();

        if len as u64 > length_constraint {
            return Err(EncodeError::Validation(ValidationError::VectorTooLong {
                count: len as u64,
                limit: length_constraint,
            }));
        }

        if T::COPY_OPTIMIZATION.is_enabled() {
            let slice = value.as_ref();
            // SAFETY: `T` has copy optimization enabled, which guarantees that it has no uninit
            // bytes and can be copied directly to the output instead of calling `encode`. This
            // means that we may cast `&[T]` to `&[u8]` and write those bytes.
            let bytes = unsafe { slice::from_raw_parts(slice.as_ptr().cast(), size_of_val(slice)) };
            encoder.write(bytes);
        } else {
            encoder.encode_next_iter_with_constraint(value.into_iter(), member_constraint)?;
        }
        OptionalVector::encode_present(out, len as u64);
    } else {
        OptionalVector::encode_absent(out);
    }
    Ok(())
}

// SAFETY: `encode_option` delegates to `encode_to_optional_vector`, which initializes the output.
unsafe impl<W, E, T> EncodeOption<OptionalVector<'static, W>, E> for Vec<T>
where
    W: Wire,
    E: Encoder + ?Sized,
    T: Encode<W, E>,
{
    fn encode_option(
        this: Option<Self>,
        encoder: &mut E,
        out: &mut MaybeUninit<OptionalVector<'static, W>>,
        constraint: VectorConstraint<W>,
    ) -> Result<(), EncodeError> {
        encode_to_optional_vector(this, encoder, out, constraint)
    }
}

// SAFETY: `encode_option` delegates to `encode_to_optional_vector`, which initializes the output.
unsafe impl<'a, W, E, T> EncodeOption<OptionalVector<'static, W>, E> for &'a Vec<T>
where
    W: Wire,
    E: Encoder + ?Sized,
    T: Encode<W, E>,
    &'a T: Encode<W, E>,
{
    fn encode_option(
        this: Option<Self>,
        encoder: &mut E,
        out: &mut MaybeUninit<OptionalVector<'static, W>>,
        constraint: VectorConstraint<W>,
    ) -> Result<(), EncodeError> {
        encode_to_optional_vector(this, encoder, out, constraint)
    }
}

// SAFETY: `encode_option` delegates to `encode_to_optional_vector`, which initializes the output.
unsafe impl<W, E, T, const N: usize> EncodeOption<OptionalVector<'static, W>, E> for [T; N]
where
    W: Wire,
    E: Encoder + ?Sized,
    T: Encode<W, E>,
{
    fn encode_option(
        this: Option<Self>,
        encoder: &mut E,
        out: &mut MaybeUninit<OptionalVector<'static, W>>,
        constraint: VectorConstraint<W>,
    ) -> Result<(), EncodeError> {
        encode_to_optional_vector(this, encoder, out, constraint)
    }
}

// SAFETY: `encode_option` delegates to `encode_to_optional_vector`, which initializes the output.
unsafe impl<'a, W, E, T, const N: usize> EncodeOption<OptionalVector<'static, W>, E> for &'a [T; N]
where
    W: Wire,
    E: Encoder + ?Sized,
    T: Encode<W, E>,
    &'a T: Encode<W, E>,
{
    fn encode_option(
        this: Option<Self>,
        encoder: &mut E,
        out: &mut MaybeUninit<OptionalVector<'static, W>>,
        constraint: VectorConstraint<W>,
    ) -> Result<(), EncodeError> {
        encode_to_optional_vector(this, encoder, out, constraint)
    }
}

// SAFETY: `encode_option` delegates to `encode_to_optional_vector`, which initializes the output.
unsafe impl<'a, W, E, T> EncodeOption<OptionalVector<'static, W>, E> for &'a [T]
where
    W: Wire,
    E: Encoder + ?Sized,
    T: Encode<W, E>,
    &'a T: Encode<W, E>,
{
    fn encode_option(
        this: Option<Self>,
        encoder: &mut E,
        out: &mut MaybeUninit<OptionalVector<'static, W>>,
        constraint: VectorConstraint<W>,
    ) -> Result<(), EncodeError> {
        encode_to_optional_vector(this, encoder, out, constraint)
    }
}

impl<T: FromWire<W>, W> FromWireOption<OptionalVector<'_, W>> for Vec<T> {
    fn from_wire_option(wire: OptionalVector<'_, W>) -> Option<Self> {
        wire.to_option().map(Vec::from_wire)
    }
}

impl<T: IntoNatural> IntoNatural for OptionalVector<'_, T> {
    type Natural = Option<Vec<T::Natural>>;
}

impl<T: FromWireRef<W>, W> FromWireOptionRef<OptionalVector<'_, W>> for Vec<T> {
    fn from_wire_option_ref(wire: &OptionalVector<'_, W>) -> Option<Self> {
        wire.as_ref().map(Vec::from_wire_ref)
    }
}

#[cfg(test)]
mod tests {
    use crate::{DecoderExt as _, EncoderExt as _, chunks, wire};

    #[test]
    fn decode_optional_vec() {
        assert_eq!(
            chunks![
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00,
            ]
            .as_mut_slice()
            .decode_with_constraint::<wire::OptionalVector<'_, wire::Uint32>>((1000, ()))
            .unwrap()
            .as_ref(),
            None,
        );
    }

    #[test]
    fn encode_optional_vec() {
        assert_eq!(
            Vec::encode_with_constraint(None::<Vec<u32>>, (1000, ())).unwrap(),
            chunks![
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00,
            ],
        );
    }
}
