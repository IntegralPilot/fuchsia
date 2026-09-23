// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The core [`Encoder`] trait.

use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::slice::from_raw_parts;

use crate::wire::Uint64;
use crate::{CHUNK_SIZE, Chunk, Encode, EncodeError, Slot, Wire};

/// An encoder for FIDL handles (internal).
pub trait InternalHandleEncoder {
    /// Returns the number of handles written to the encoder.
    ///
    /// This method exposes details about Fuchsia resources that plain old FIDL shouldn't need to
    /// know about. Do not use this method outside of this crate.
    #[doc(hidden)]
    fn __internal_handle_count(&self) -> usize;
}

/// An encoder for FIDL messages.
pub trait Encoder: InternalHandleEncoder {
    /// Returns the number of bytes written to the encoder.
    fn bytes_written(&self) -> usize;

    /// Writes zeroed bytes to the end of the encoder.
    ///
    /// Additional bytes are written to pad the written data to a multiple of [`CHUNK_SIZE`].
    fn write_zeroes(&mut self, len: usize);

    /// Copies bytes to the end of the encoder.
    ///
    /// Additional bytes are written to pad the written data to a multiple of [`CHUNK_SIZE`].
    fn write(&mut self, bytes: &[u8]);

    /// Rewrites bytes at a position in the encoder.
    fn rewrite(&mut self, pos: usize, bytes: &[u8]);
}

impl InternalHandleEncoder for Vec<Chunk> {
    #[inline]
    fn __internal_handle_count(&self) -> usize {
        0
    }
}

impl Encoder for Vec<Chunk> {
    #[inline]
    fn bytes_written(&self) -> usize {
        self.len() * CHUNK_SIZE
    }

    #[inline]
    fn write_zeroes(&mut self, len: usize) {
        let count = len.div_ceil(CHUNK_SIZE);
        self.reserve(count);
        // SAFETY: `reserve` ensures the vector has enough capacity for `count` additional
        // elements.
        let ptr = unsafe { self.as_mut_ptr().add(self.len()) };
        // SAFETY: `ptr` is valid for writing `count` elements because of the previous `reserve`
        // call.
        unsafe {
            ptr.write_bytes(0, count);
        }
        // SAFETY: The memory up to the new length has been initialized to zero.
        unsafe {
            self.set_len(self.len() + count);
        }
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let count = bytes.len().div_ceil(CHUNK_SIZE);
        self.reserve(count);

        // Zero out the last chunk
        // SAFETY: `reserve` ensures the pointer is within the allocated capacity.
        unsafe {
            self.as_mut_ptr().add(self.len() + count - 1).write(Uint64(0));
        }
        // SAFETY: `reserve` ensures the pointer is within the allocated capacity.
        let ptr = unsafe { self.as_mut_ptr().add(self.len()).cast::<u8>() };

        // Copy all the bytes
        // SAFETY: `ptr` has sufficient capacity for `bytes.len()`,
        // which is less than or equal to `count * CHUNK_SIZE`.
        unsafe {
            ptr.copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
        }

        // Set the new length
        // SAFETY: All `count` chunks have been initialized.
        unsafe {
            self.set_len(self.len() + count);
        }
    }

    #[inline]
    fn rewrite(&mut self, pos: usize, bytes: &[u8]) {
        assert!(pos + bytes.len() <= self.bytes_written());

        // SAFETY: `pos` is within the initialized bounds of the vector.
        let ptr = unsafe { self.as_mut_ptr().cast::<u8>().add(pos) };
        // SAFETY: The destination pointer is valid for writes of `bytes.len()` and
        // does not overlap with `bytes`.
        unsafe {
            ptr.copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
        }
    }
}

/// Extension methods for [`Encoder`].
pub trait EncoderExt {
    /// Pre-allocates space for a slice of elements.
    fn preallocate<T>(&mut self, len: usize) -> Preallocated<'_, Self, T>;

    /// Encodes an iterator of elements.
    ///
    /// Returns `Err` if encoding failed.
    fn encode_next_iter<W, T>(
        &mut self,
        values: impl ExactSizeIterator<Item = T>,
    ) -> Result<(), EncodeError>
    where
        W: Wire<Constraint = ()>,
        T: Encode<W, Self>;

    /// Encodes an iterator of elements.
    ///
    /// Returns `Err` if encoding failed.
    fn encode_next_iter_with_constraint<W, T>(
        &mut self,
        values: impl ExactSizeIterator<Item = T>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError>
    where
        W: Wire,
        T: Encode<W, Self>;

    /// Encodes a value.
    ///
    /// Returns `Err` if encoding failed.
    fn encode_next<W, T>(&mut self, value: T) -> Result<(), EncodeError>
    where
        W: Wire<Constraint = ()>,
        T: Encode<W, Self>;

    /// Encodes a value with a constraint.
    ///
    /// Returns `Err` if encoding failed.
    fn encode_next_with_constraint<W: Wire, T: Encode<W, Self>>(
        &mut self,
        value: T,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError>;

    /// Encodes a value into a new instance of the encoder.
    ///
    /// Returns `Err` if encoding failed.
    fn encode<W, T>(value: T) -> Result<Self, EncodeError>
    where
        Self: Default,
        W: Wire<Constraint = ()>,
        T: Encode<W, Self>;

    /// Encodes a value with a constraint into a new instance of the encoder.
    ///
    /// Returns `Err` if encoding failed.
    fn encode_with_constraint<W, T>(
        value: T,
        constraint: W::Constraint,
    ) -> Result<Self, EncodeError>
    where
        Self: Default,
        W: Wire,
        T: Encode<W, Self>;
}

impl<E: Encoder + ?Sized> EncoderExt for E {
    fn preallocate<T>(&mut self, len: usize) -> Preallocated<'_, Self, T> {
        let pos = self.bytes_written();

        // Zero out the next `count` bytes
        self.write_zeroes(len * size_of::<T>());

        Preallocated {
            encoder: self,
            pos,
            #[cfg(debug_assertions)]
            remaining: len,
            _phantom: PhantomData,
        }
    }

    fn encode_next_iter<W, T>(
        &mut self,
        values: impl ExactSizeIterator<Item = T>,
    ) -> Result<(), EncodeError>
    where
        W: Wire<Constraint = ()>,
        T: Encode<W, Self>,
    {
        self.encode_next_iter_with_constraint(values, ())
    }

    fn encode_next_iter_with_constraint<W, T>(
        &mut self,
        values: impl ExactSizeIterator<Item = T>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError>
    where
        W: Wire,
        T: Encode<W, Self>,
    {
        let mut outputs = self.preallocate::<W>(values.len());

        let mut out = MaybeUninit::<W>::uninit();
        <W as Wire>::zero_padding(&mut out);
        for value in values {
            value.encode(outputs.encoder, &mut out, constraint)?;
            // SAFETY: `out` has been fully initialized by `W::zero_padding` and `value.encode`.
            W::validate(unsafe { Slot::new_unchecked_from_maybe_uninit(&mut out) }, constraint)
                .map_err(EncodeError::Validation)?;
            // SAFETY: `out` has been fully initialized.
            unsafe {
                outputs.write_next(out.assume_init_ref());
            }
        }

        Ok(())
    }

    fn encode_next<W, T>(&mut self, value: T) -> Result<(), EncodeError>
    where
        W: Wire<Constraint = ()>,
        T: Encode<W, Self>,
    {
        self.encode_next_with_constraint(value, ())
    }

    fn encode_next_with_constraint<W, T>(
        &mut self,
        value: T,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError>
    where
        W: Wire,
        T: Encode<W, Self>,
    {
        self.encode_next_iter_with_constraint(core::iter::once(value), constraint)
    }

    fn encode<W, T>(value: T) -> Result<Self, EncodeError>
    where
        Self: Default,
        W: Wire<Constraint = ()>,
        T: Encode<W, Self>,
    {
        Self::encode_with_constraint(value, ())
    }

    fn encode_with_constraint<W, T>(
        value: T,
        constraint: W::Constraint,
    ) -> Result<Self, EncodeError>
    where
        Self: Default,
        W: Wire,
        T: Encode<W, Self>,
    {
        let mut result = Self::default();
        result.encode_next_with_constraint(value, constraint)?;
        Ok(result)
    }
}

/// A pre-allocated slice of elements
pub struct Preallocated<'a, E: ?Sized, T> {
    /// The encoder.
    pub encoder: &'a mut E,
    pos: usize,
    #[cfg(debug_assertions)]
    remaining: usize,
    _phantom: PhantomData<T>,
}

impl<E: Encoder + ?Sized, T> Preallocated<'_, E, T> {
    /// Writes into the next pre-allocated slot in the encoder.
    ///
    /// # Safety
    ///
    /// All of the bytes of `value` must be initialized, including padding.
    pub unsafe fn write_next(&mut self, value: &T) {
        #[cfg(debug_assertions)]
        {
            assert!(self.remaining > 0, "attemped to write more slots than preallocated");
            self.remaining -= 1;
        }

        let bytes_ptr = (value as *const T).cast::<u8>();
        // SAFETY: `value` is valid for reads of `size_of::<T>()` bytes, and the
        // caller guarantees it is fully initialized.
        let bytes = unsafe { from_raw_parts(bytes_ptr, size_of::<T>()) };
        self.encoder.rewrite(self.pos, bytes);
        self.pos += size_of::<T>();
    }
}
