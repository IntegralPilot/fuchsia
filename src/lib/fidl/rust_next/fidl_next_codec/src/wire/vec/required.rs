// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;
use core::mem::{MaybeUninit, forget, needs_drop};
use core::ops::Deref;
use core::ptr::{NonNull, copy_nonoverlapping};
use core::{fmt, slice};

use munge::munge;

use super::raw::RawVector;
use crate::{
    Chunk, Constrained, Decode, DecodeError, Decoder, DecoderExt as _, Encode, EncodeError,
    Encoder, EncoderExt as _, FromWire, FromWireRef, IntoNatural, Slot, ValidationError, Wire,
    wire,
};

/// A FIDL vector
#[repr(transparent)]
pub struct Vector<'de, T> {
    raw: RawVector<'de, T>,
}

// SAFETY: `Vector` is `repr(transparent)` over `RawVector`, which implements `Wire`.
// Lifetime erasure is safe since `Vector` is covariant over its lifetime.
unsafe impl<T: Wire> Wire for Vector<'static, T> {
    type Narrowed<'de> = Vector<'de, T::Narrowed<'de>>;

    #[inline]
    fn zero_padding(out: &mut MaybeUninit<Self>) {
        munge!(let Self { raw } = out);
        RawVector::<T>::zero_padding(raw);
    }
}

impl<T> Drop for Vector<'_, T> {
    fn drop(&mut self) {
        if needs_drop::<T>() {
            // SAFETY: If `T` needs to be dropped, the pointer has been decoded and points to a
            // valid slice of initialized `T` elements.
            unsafe {
                self.raw.as_slice_ptr().drop_in_place();
            }
        }
    }
}

impl<'de, T> Vector<'de, T> {
    /// Encodes that a vector is present in a slot.
    pub fn encode_present(out: &mut MaybeUninit<Self>, len: u64) {
        munge!(let Self { raw } = out);
        RawVector::encode_present(raw, len);
    }

    /// Returns the length of the vector in elements.
    pub fn len(&self) -> usize {
        self.raw.len() as usize
    }

    /// Returns whether the vector is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns a pointer to the elements of the vector.
    fn as_slice_ptr(&self) -> NonNull<[T]> {
        // SAFETY: The underlying pointer is guaranteed to be non-null for a valid `Vector`.
        unsafe { NonNull::new_unchecked(self.raw.as_slice_ptr()) }
    }

    /// Returns a slice of the elements of the vector.
    pub fn as_slice(&self) -> &[T] {
        // SAFETY: The pointer is aligned, initialized, and the lifetime of the reference
        // is bound to the lifetime of `self`.
        unsafe { self.as_slice_ptr().as_ref() }
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

        if !wire::Pointer::is_encoded_present(ptr.as_mut())? {
            return Err(DecodeError::RequiredValueAbsent);
        }

        if **len > max_len {
            return Err(DecodeError::Validation(ValidationError::VectorTooLong {
                count: **len,
                limit: max_len,
            }));
        }

        let slice = decoder.take_slice_slot::<T>(**len as usize)?;
        wire::Pointer::set_decoded_slice(ptr, slice);

        Ok(())
    }

    /// Validate that this vector's length falls within the limit.
    pub(crate) fn validate_max_len(
        slot: Slot<'_, Self>,
        limit: u64,
    ) -> Result<(), ValidationError> {
        munge!(let Self { raw: RawVector { len, ptr:_ } } = slot);
        let count: u64 = **len;
        if count > limit { Err(ValidationError::VectorTooLong { count, limit }) } else { Ok(()) }
    }
}

type VectorConstraint<T> = (u64, <T as Constrained>::Constraint);

impl<T: Constrained> Constrained for Vector<'_, T> {
    type Constraint = VectorConstraint<T>;

    fn validate(slot: Slot<'_, Self>, constraint: Self::Constraint) -> Result<(), ValidationError> {
        let (limit, _) = constraint;

        munge!(let Self { raw: RawVector { len, ptr:_ } } = slot);
        let count = **len;
        if count > limit {
            return Err(ValidationError::VectorTooLong { count, limit });
        }

        Ok(())
    }
}

/// An iterator over the items of a `WireVector`.
pub struct IntoIter<'de, T> {
    current: *mut T,
    remaining: usize,
    _phantom: PhantomData<&'de mut [Chunk]>,
}

impl<T> Drop for IntoIter<'_, T> {
    fn drop(&mut self) {
        for i in 0..self.remaining {
            // SAFETY: `self.current.add(i)` points to an initialized element of `T` within the
            // original vector's allocation that has not yet been yielded by the iterator.
            unsafe {
                self.current.add(i).drop_in_place();
            }
        }
    }
}

impl<T> Iterator for IntoIter<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            None
        } else {
            // SAFETY: `self.current` points to a valid, initialized element of `T` that has not
            // yet been read. We ownership-transfer it, and decrement `self.remaining` to ensure
            // it is not dropped again.
            let result = unsafe { self.current.read() };
            // SAFETY: `self.current` is within the bounds of the allocated slice, so advancing
            // it by 1 is safe (it may point to one-past-the-end if it was the last element).
            self.current = unsafe { self.current.add(1) };
            self.remaining -= 1;
            Some(result)
        }
    }
}

impl<'de, T> IntoIterator for Vector<'de, T> {
    type IntoIter = IntoIter<'de, T>;
    type Item = T;

    fn into_iter(self) -> Self::IntoIter {
        let current = self.raw.as_ptr();
        let remaining = self.len();
        forget(self);

        IntoIter { current, remaining, _phantom: PhantomData }
    }
}

impl<T> Deref for Vector<'_, T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl<T: fmt::Debug> fmt::Debug for Vector<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_slice().fmt(f)
    }
}

impl<T, U: ?Sized> PartialEq<&U> for Vector<'_, T>
where
    for<'de> Vector<'de, T>: PartialEq<U>,
{
    fn eq(&self, other: &&U) -> bool {
        self == *other
    }
}

impl<T: PartialEq<U>, U, const N: usize> PartialEq<[U; N]> for Vector<'_, T> {
    fn eq(&self, other: &[U; N]) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: PartialEq<U>, U> PartialEq<[U]> for Vector<'_, T> {
    fn eq(&self, other: &[U]) -> bool {
        self.as_slice() == other
    }
}

impl<T: PartialEq<U>, U> PartialEq<Vector<'_, U>> for Vector<'_, T> {
    fn eq(&self, other: &Vector<'_, U>) -> bool {
        self.as_slice() == other.as_slice()
    }
}

// SAFETY: If `decode` returns `Ok`, the `Vector` has been successfully decoded,
// and the underlying pointer is updated to point to a successfully decoded slice
// of `T` allocated by the decoder.
unsafe impl<'de, D, T> Decode<D> for Vector<'de, T>
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

        if **len > length_constraint {
            return Err(DecodeError::Validation(ValidationError::VectorTooLong {
                count: **len,
                limit: length_constraint,
            }));
        }

        if !wire::Pointer::is_encoded_present(ptr.as_mut())? {
            return Err(DecodeError::RequiredValueAbsent);
        }

        let mut slice = decoder.take_slice_slot::<T>(**len as usize)?;
        for i in 0..**len as usize {
            T::decode(slice.index(i), decoder, member_constraint)?;
        }
        wire::Pointer::set_decoded_slice(ptr, slice);

        Ok(())
    }
}

#[inline]
fn encode_to_vector<V, W, E, T>(
    value: V,
    encoder: &mut E,
    out: &mut MaybeUninit<Vector<'static, W>>,
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
    let len = value.as_ref().len();
    let (length_constraint, member_constraint) = constraint;

    if len as u64 > length_constraint {
        return Err(EncodeError::Validation(ValidationError::VectorTooLong {
            count: len as u64,
            limit: length_constraint,
        }));
    }

    if T::COPY_OPTIMIZATION.is_enabled() {
        let slice = value.as_ref();
        // SAFETY: `T` has copy optimization enabled, which guarantees that it has no uninit bytes
        // and can be copied directly to the output instead of calling `encode`. This means that we
        // may cast `&[T]` to `&[u8]` and write those bytes.
        let bytes = unsafe { slice::from_raw_parts(slice.as_ptr().cast(), size_of_val(slice)) };
        encoder.write(bytes);
        // TODO: copy-optimized encodings don't currently check constraints
    } else {
        encoder.encode_next_iter_with_constraint(value.into_iter(), member_constraint)?;
    }
    Vector::encode_present(out, len as u64);
    Ok(())
}

// SAFETY: `encode` delegates to `encode_to_vector`, which initializes the output.
unsafe impl<W, E, T> Encode<Vector<'static, W>, E> for Vec<T>
where
    W: Wire,
    E: Encoder + ?Sized,
    T: Encode<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<Vector<'static, W>>,
        constraint: VectorConstraint<W>,
    ) -> Result<(), EncodeError> {
        encode_to_vector(self, encoder, out, constraint)
    }
}

// SAFETY: `encode` delegates to `encode_to_vector`, which initializes the output.
unsafe impl<'a, W, E, T> Encode<Vector<'static, W>, E> for &'a Vec<T>
where
    W: Wire,
    E: Encoder + ?Sized,
    T: Encode<W, E>,
    &'a T: Encode<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<Vector<'static, W>>,
        constraint: VectorConstraint<W>,
    ) -> Result<(), EncodeError> {
        encode_to_vector(self, encoder, out, constraint)
    }
}

// SAFETY: `encode` delegates to `encode_to_vector`, which initializes the output.
unsafe impl<W, E, T, const N: usize> Encode<Vector<'static, W>, E> for [T; N]
where
    W: Wire,
    E: Encoder + ?Sized,
    T: Encode<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<Vector<'static, W>>,
        constraint: VectorConstraint<W>,
    ) -> Result<(), EncodeError> {
        encode_to_vector(self, encoder, out, constraint)
    }
}

// SAFETY: `encode` delegates to `encode_to_vector`, which initializes the output.
unsafe impl<'a, W, E, T, const N: usize> Encode<Vector<'static, W>, E> for &'a [T; N]
where
    W: Wire,
    E: Encoder + ?Sized,
    T: Encode<W, E>,
    &'a T: Encode<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<Vector<'static, W>>,
        constraint: VectorConstraint<W>,
    ) -> Result<(), EncodeError> {
        encode_to_vector(self, encoder, out, constraint)
    }
}

// SAFETY: `encode` delegates to `encode_to_vector`, which initializes the output.
unsafe impl<'a, W, E, T> Encode<Vector<'static, W>, E> for &'a [T]
where
    W: Wire,
    E: Encoder + ?Sized,
    T: Encode<W, E>,
    &'a T: Encode<W, E>,
{
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<Vector<'static, W>>,
        constraint: VectorConstraint<W>,
    ) -> Result<(), EncodeError> {
        encode_to_vector(self, encoder, out, constraint)
    }
}

impl<T: FromWire<W>, W> FromWire<Vector<'_, W>> for Vec<T> {
    fn from_wire(wire: Vector<'_, W>) -> Self {
        let mut result = Vec::<T>::with_capacity(wire.len());
        if T::COPY_OPTIMIZATION.is_enabled() {
            // SAFETY: `T` has copy optimization enabled, meaning it is layout-compatible with `W`
            // and can be safely copied. The destination buffer has been allocated with sufficient
            // capacity, and the source and destination do not overlap.
            unsafe {
                copy_nonoverlapping(wire.as_ptr().cast(), result.as_mut_ptr(), wire.len());
            }
            // SAFETY: We have just initialized the first `wire.len()` elements of `result`
            // via `copy_nonoverlapping`.
            unsafe {
                result.set_len(wire.len());
            }
            forget(wire);
        } else {
            for item in wire.into_iter() {
                result.push(T::from_wire(item));
            }
        }
        result
    }
}

impl<T: IntoNatural> IntoNatural for Vector<'_, T> {
    type Natural = Vec<T::Natural>;
}

impl<T: FromWireRef<W>, W> FromWireRef<Vector<'_, W>> for Vec<T> {
    fn from_wire_ref(wire: &Vector<'_, W>) -> Self {
        let mut result = Vec::<T>::with_capacity(wire.len());
        if T::COPY_OPTIMIZATION.is_enabled() {
            // SAFETY: `T` has copy optimization enabled, meaning it is layout-compatible with `W`
            // and can be safely copied. The destination buffer has been allocated with sufficient
            // capacity, and the source and destination do not overlap.
            unsafe {
                copy_nonoverlapping(wire.as_ptr().cast(), result.as_mut_ptr(), wire.len());
            }
            // SAFETY: We have just initialized the first `wire.len()` elements of `result`
            // via `copy_nonoverlapping`.
            unsafe {
                result.set_len(wire.len());
            }
        } else {
            for item in wire.iter() {
                result.push(T::from_wire_ref(item));
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::{DecoderExt as _, EncoderExt as _, chunks, wire};

    #[test]
    fn decode_vec() {
        assert_eq!(
            chunks![
                0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0x78, 0x56, 0x34, 0x12, 0xf0, 0xde, 0xbc, 0x9a,
            ]
            .as_mut_slice()
            .decode_with_constraint::<wire::Vector<'_, wire::Uint32>>((1000, ()))
            .unwrap()
            .as_slice(),
            &[wire::Uint32(0x12345678), wire::Uint32(0x9abcdef0)],
        );
        assert_eq!(
            chunks![
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff,
            ]
            .as_mut_slice()
            .decode_with_constraint::<wire::Vector<'_, wire::Uint32>>((1000, ()))
            .unwrap()
            .as_ref(),
            <[wire::Uint32; _]>::as_slice(&[]),
        );
    }

    #[test]
    fn encode_vec() {
        assert_eq!(
            Vec::encode_with_constraint(Some(vec![0x12345678u32, 0x9abcdef0u32]), (1000, ()))
                .unwrap(),
            chunks![
                0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0x78, 0x56, 0x34, 0x12, 0xf0, 0xde, 0xbc, 0x9a,
            ],
        );
        assert_eq!(
            Vec::encode_with_constraint(Some(Vec::<u32>::new()), (1000, ())).unwrap(),
            chunks![
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff,
            ],
        );
    }
}
