// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The core [`Decoder`] trait.

use core::mem::take;

use crate::{CHUNK_SIZE, Chunk, Decode, DecodeError, Slot};

/// A decoder for FIDL handles (internal).
pub trait InternalHandleDecoder {
    /// Takes the next `count` handles from the decoder.
    ///
    /// This method exposes details about Fuchsia resources that plain old FIDL shouldn't need to
    /// know about. Do not use this method outside of this crate.
    #[doc(hidden)]
    fn __internal_take_handles(&mut self, count: usize) -> Result<(), DecodeError>;

    /// Returns the number of handles remaining in the decoder.
    ///
    /// This method exposes details about Fuchsia resources that plain old FIDL shouldn't need to
    /// know about. Do not use this method outside of this crate.
    #[doc(hidden)]
    fn __internal_handles_remaining(&self) -> usize;
}

/// A decoder for FIDL messages.
pub trait Decoder<'de>: InternalHandleDecoder {
    /// Takes a slice of `Chunk`s from the decoder.
    fn take_chunks(&mut self, count: usize) -> Result<&'de mut [Chunk], DecodeError>;

    /// Commits to any decoding operations which are in progress.
    ///
    /// Resources like handles may be taken from a decoder during decoding. However, decoding may
    /// fail after those resources are taken but before decoding completes. To ensure that resources
    /// are always dropped, taken resources are still considered owned by the decoder until `commit`
    /// is called. After `commit`, ownership of those resources is transferred to the decoded data.
    fn commit(&mut self);

    /// Verifies that decoding finished cleanly, with no leftover chunks or resources.
    fn finish(&self) -> Result<(), DecodeError>;
}

impl InternalHandleDecoder for &mut [Chunk] {
    #[inline]
    fn __internal_take_handles(&mut self, _: usize) -> Result<(), DecodeError> {
        Err(DecodeError::InsufficientHandles)
    }

    #[inline]
    fn __internal_handles_remaining(&self) -> usize {
        0
    }
}

impl<'de> Decoder<'de> for &'de mut [Chunk] {
    #[inline]
    fn take_chunks(&mut self, count: usize) -> Result<&'de mut [Chunk], DecodeError> {
        if count > self.len() {
            return Err(DecodeError::InsufficientData);
        }

        let chunks = take(self);
        // SAFETY: We just checked that `count <= self.len()`.
        let (prefix, suffix) = unsafe { chunks.split_at_mut_unchecked(count) };
        *self = suffix;
        Ok(prefix)
    }

    #[inline]
    fn commit(&mut self) {
        // No resources to take, so commit is a no-op
    }

    #[inline]
    fn finish(&self) -> Result<(), DecodeError> {
        if !self.is_empty() {
            return Err(DecodeError::ExtraBytes { num_extra: self.len() * CHUNK_SIZE });
        }

        Ok(())
    }
}

/// Extension methods for [`Decoder`].
pub trait DecoderExt<'de> {
    /// Takes enough chunks for a `T`, returning a `Slot` of the taken value.
    fn take_slot<T>(&mut self) -> Result<Slot<'de, T>, DecodeError>;

    /// Takes enough chunks for a slice of `T`, returning a `Slot` of the taken
    /// slice.
    fn take_slice_slot<T>(&mut self, len: usize) -> Result<Slot<'de, [T]>, DecodeError>;

    /// Decodes an owned value from the decoder without finishing it.
    ///
    /// On success, returns `Ok` of an owned value. Returns `Err` if decoding
    /// failed.
    fn decode_prefix<T>(&mut self) -> Result<T, DecodeError>
    where
        T: Decode<Self, Constraint = ()>;

    /// Decodes an owned value from the decoder with some constraint without
    /// finishing it.
    ///
    /// On success, returns `Ok` of an owned value. Returns `Err` if decoding
    /// failed.
    fn decode_prefix_with_constraint<T>(
        &mut self,
        constraint: T::Constraint,
    ) -> Result<T, DecodeError>
    where
        T: Decode<Self>;

    /// Decodes an owned value from the decoder and finishes it.
    ///
    /// On success, returns `Ok` of an owned value. Returns `Err` if decoding
    /// failed.
    fn decode<T>(&mut self) -> Result<T, DecodeError>
    where
        T: Decode<Self, Constraint = ()>;

    /// Decodes an owned value from the decoder with some constraint and
    /// finishes it.
    ///
    /// On success, returns `Ok` of an owned value. Returns `Err` if decoding
    /// failed.
    fn decode_with_constraint<T>(&mut self, constraint: T::Constraint) -> Result<T, DecodeError>
    where
        T: Decode<Self>;
}

impl<'de, D: Decoder<'de> + ?Sized> DecoderExt<'de> for D {
    fn take_slot<T>(&mut self) -> Result<Slot<'de, T>, DecodeError> {
        // TODO: might be able to move this into a const for guaranteed const
        // eval
        assert!(
            align_of::<T>() <= CHUNK_SIZE,
            "attempted to take a slot for a type with an alignment higher \
             than {CHUNK_SIZE}",
        );

        let count = size_of::<T>().div_ceil(CHUNK_SIZE);
        let chunks = self.take_chunks(count)?;
        // SAFETY: `result` is at least 8-aligned and points to at least enough
        // bytes for a `T`.
        unsafe { Ok(Slot::new_unchecked(chunks.as_mut_ptr().cast())) }
    }

    fn take_slice_slot<T>(&mut self, len: usize) -> Result<Slot<'de, [T]>, DecodeError> {
        assert!(
            align_of::<T>() <= CHUNK_SIZE,
            "attempted to take a slice slot for a type with an alignment \
             higher than {CHUNK_SIZE}",
        );

        let slice_byte_length =
            size_of::<T>().checked_mul(len).ok_or(DecodeError::InsufficientData)?;
        let chunk_count = slice_byte_length.div_ceil(CHUNK_SIZE);
        let chunk_length =
            chunk_count.checked_mul(CHUNK_SIZE).ok_or(DecodeError::InsufficientData)?;
        let padding_length = chunk_length - slice_byte_length;
        let chunks_ptr = self.take_chunks(chunk_count)?.as_mut_ptr();
        // SAFETY: The padding pointer falls within the chunks returned by `take_chunks`,
        // which we have exclusive access to.
        let padding: &[u8] = unsafe {
            core::slice::from_raw_parts(
                chunks_ptr.cast::<u8>().add(slice_byte_length),
                padding_length,
            )
        };
        if padding.iter().any(|byte| *byte != 0) {
            return Err(DecodeError::InvalidPadding);
        }

        // SAFETY: `result` is at least 8-aligned and points to at least enough
        // bytes for a slice of `T` of length `len`.
        unsafe { Ok(Slot::new_slice_unchecked(chunks_ptr.cast(), len)) }
    }

    fn decode_prefix<T>(&mut self) -> Result<T, DecodeError>
    where
        T: Decode<Self, Constraint = ()>,
    {
        self.decode_prefix_with_constraint(())
    }

    fn decode_prefix_with_constraint<T>(
        &mut self,
        constraint: T::Constraint,
    ) -> Result<T, DecodeError>
    where
        T: Decode<Self>,
    {
        let mut slot = self.take_slot::<T>()?;
        T::decode(slot.as_mut(), self, constraint)?;
        self.commit();
        // SAFETY: `slot` decoded successfully and the decoder was committed. `slot` now points to
        // a valid `T` within the decoder.
        unsafe { Ok(slot.as_mut_ptr().read()) }
    }

    fn decode<T>(&mut self) -> Result<T, DecodeError>
    where
        T: Decode<Self, Constraint = ()>,
    {
        self.decode_with_constraint(())
    }

    fn decode_with_constraint<T>(&mut self, constraint: T::Constraint) -> Result<T, DecodeError>
    where
        T: Decode<Self>,
    {
        let result = self.decode_prefix_with_constraint(constraint)?;
        self.finish()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_take_slice_slot_integer_overflow() {
        let mut chunks = [Chunk::default(); 4];
        let mut decoder: &mut [Chunk] = &mut chunks;

        // A ridiculously large length that would cause overflow when multiplied by size_of::<u64>() (which is 8)
        let len = usize::MAX / 4;

        // This should return an Err containing DecodeError::InsufficientData instead of overflowing or panicking.
        let result = decoder.take_slice_slot::<u64>(len);
        assert!(matches!(result, Err(DecodeError::InsufficientData)));
    }
}
