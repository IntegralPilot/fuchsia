// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::{ManuallyDrop, MaybeUninit};
use core::ptr::addr_of_mut;

use munge::munge;

use crate::decoder::InternalHandleDecoder;
use crate::encoder::InternalHandleEncoder;
use crate::{
    CHUNK_SIZE, Constrained, Decode, DecodeError, Decoder, DecoderExt as _, Encode, EncodeError,
    Encoder, EncoderExt as _, Slot, ValidationError, Wire, wire,
};

#[derive(Clone, Copy)]
#[repr(C)]
struct Encoded {
    maybe_num_bytes: wire::Uint32,
    num_handles: wire::Uint16,
    flags: wire::Uint16,
}

const INLINE_SIZE: usize = 4;

/// A FIDL envelope
#[repr(C, align(8))]
pub union Envelope {
    zero: [u8; 8],
    encoded: Encoded,
    decoded_inline: [MaybeUninit<u8>; INLINE_SIZE],
    decoded_out_of_line: *mut (),
}

// SAFETY: `Envelope` is a union of primitive types and pointers, and contains no thread-local
// data.
unsafe impl Send for Envelope {}
// SAFETY: `Envelope` contains no interior mutability.
unsafe impl Sync for Envelope {}

impl Constrained for Envelope {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

// SAFETY: `Envelope` has a stable layout and no padding.
unsafe impl Wire for Envelope {
    type Narrowed<'de> = Self;

    fn zero_padding(_: &mut MaybeUninit<Self>) {}
}

impl Envelope {
    const IS_INLINE_BIT: u16 = 1;

    /// Encodes a zero envelope into a slot.
    #[inline]
    pub fn encode_zero(out: &mut MaybeUninit<Self>) {
        out.write(Envelope { zero: [0; 8] });
    }

    /// Encodes a `'static` value into an envelope with an encoder.
    #[inline]
    pub fn encode_value_static<W: Wire, E: InternalHandleEncoder + ?Sized>(
        value: impl Encode<W, E>,
        encoder: &mut E,
        out: &mut MaybeUninit<Self>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        // `unsafe` block required in the next version of munge
        #[allow(unused_unsafe)]
        // SAFETY: `out` is a valid mutable reference to a `MaybeUninit<Envelope>`.
        // Destructuring it via `munge!` is safe.
        let encoded = unsafe {
            munge!(let Self { encoded } = out);
            encoded
        };
        munge! {
            let Encoded {
                maybe_num_bytes,
                num_handles,
                flags,
            } = encoded;
        }

        let handles_before = encoder.__internal_handle_count();

        let encoded_size = size_of::<W>();
        if encoded_size <= INLINE_SIZE {
            // If the encoded inline value is less than 4 bytes long, we need to zero out the part
            // that won't get written over
            // SAFETY: `encoded_size` is <= INLINE_SIZE (4), so we are writing within the
            // bounds of `maybe_num_bytes` (which is 4 bytes).
            unsafe {
                maybe_num_bytes
                    .as_mut_ptr()
                    .cast::<u8>()
                    .add(encoded_size)
                    .write_bytes(0, INLINE_SIZE - encoded_size);
            }
        } else {
            return Err(EncodeError::ExpectedInline(encoded_size));
        }

        // SAFETY: `maybe_num_bytes` points to a 4-byte slot. `W` has size <= 4 and alignment
        // requirements that are compatible (since `Envelope` is aligned to 8).
        // Casting it to `*mut W` and dereferencing it is safe because the memory is allocated
        // and we have exclusive access.
        let value_out = unsafe { &mut *maybe_num_bytes.as_mut_ptr().cast() };
        W::zero_padding(value_out);
        value.encode(encoder, value_out, constraint)?;

        flags.write(wire::Uint16(Self::IS_INLINE_BIT));

        let handle_count = (encoder.__internal_handle_count() - handles_before).try_into().unwrap();
        num_handles.write(wire::Uint16(handle_count));

        Ok(())
    }

    /// Encodes a value into an envelope with an encoder.
    #[inline]
    pub fn encode_value<W: Wire, E: Encoder + ?Sized>(
        value: impl Encode<W, E>,
        encoder: &mut E,
        out: &mut MaybeUninit<Self>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        // `unsafe` block required in the next version of munge
        #[allow(unused_unsafe)]
        // SAFETY: `out` is a valid mutable reference to a `MaybeUninit<Envelope>`.
        // Destructuring it via `munge!` is safe.
        let encoded = unsafe {
            munge!(let Self { encoded } = out);
            encoded
        };
        munge! {
            let Encoded {
                maybe_num_bytes,
                num_handles,
                flags,
            } = encoded;
        }

        let handles_before = encoder.__internal_handle_count();

        let encoded_size = size_of::<W>();
        if encoded_size <= INLINE_SIZE {
            // If the encoded inline value is less than 4 bytes long, we need to zero out the part
            // that won't get written over
            // SAFETY: `encoded_size` is <= INLINE_SIZE (4), so we are writing within the
            // bounds of `maybe_num_bytes` (which is 4 bytes).
            unsafe {
                maybe_num_bytes
                    .as_mut_ptr()
                    .cast::<u8>()
                    .add(encoded_size)
                    .write_bytes(0, INLINE_SIZE - encoded_size);
            }
            // SAFETY: `maybe_num_bytes` points to a 4-byte slot. `W` has size <= 4 and alignment
            // requirements that are compatible (since `Envelope` is aligned to 8).
            // Casting it to `*mut W` and dereferencing it is safe because the memory is allocated
            // and we have exclusive access.
            let value_out = unsafe { &mut *maybe_num_bytes.as_mut_ptr().cast() };
            W::zero_padding(value_out);
            value.encode(encoder, value_out, constraint)?;
            flags.write(wire::Uint16(Self::IS_INLINE_BIT));
        } else {
            let bytes_before = encoder.bytes_written();

            encoder.encode_next_with_constraint(value, constraint)?;

            let bytes_count = (encoder.bytes_written() - bytes_before).try_into().unwrap();
            maybe_num_bytes.write(wire::Uint32(bytes_count));
            flags.write(wire::Uint16(0));
        }

        let handle_count = (encoder.__internal_handle_count() - handles_before).try_into().unwrap();
        num_handles.write(wire::Uint16(handle_count));

        Ok(())
    }

    /// Returns the zero envelope.
    #[inline]
    pub fn zero() -> Self {
        Self { zero: [0; 8] }
    }

    /// Returns whether a envelope slot is encoded as zero.
    #[inline]
    pub fn is_encoded_zero(slot: Slot<'_, Self>) -> bool {
        // `unsafe` block required in the next version of munge
        #[allow(unused_unsafe)]
        // SAFETY: `slot` is a valid `Slot` of `Envelope`. Destructuring it is safe.
        let zero = unsafe {
            munge!(let Self { zero } = slot);
            zero
        };
        *zero == [0; 8]
    }

    /// Returns whether an envelope is zero.
    #[inline]
    pub fn is_zero(&self) -> bool {
        // SAFETY: Reading the `zero` field of the union is safe because it is a primitive array
        // (`[u8; 8]`) which has no validity invariants.
        unsafe { self.zero == [0; 8] }
    }

    #[inline]
    fn out_of_line_chunks(
        maybe_num_bytes: Slot<'_, wire::Uint32>,
        flags: Slot<'_, wire::Uint16>,
    ) -> Result<Option<usize>, DecodeError> {
        match **flags {
            Self::IS_INLINE_BIT => Ok(None),
            0 => {
                let num_bytes = **maybe_num_bytes;
                if !(num_bytes as usize).is_multiple_of(CHUNK_SIZE) {
                    Err(DecodeError::InvalidEnvelopeSize(num_bytes))
                } else if num_bytes <= INLINE_SIZE as u32 {
                    Err(DecodeError::OutOfLineValueTooSmall(num_bytes))
                } else {
                    Ok(Some(num_bytes as usize / CHUNK_SIZE))
                }
            }
            _ => Err(DecodeError::InvalidEnvelopeFlags(**flags)),
        }
    }

    /// Decodes and discards a static type in an envelope.
    #[inline]
    pub fn decode_unknown_static<D: InternalHandleDecoder + ?Sized>(
        slot: Slot<'_, Self>,
        decoder: &mut D,
    ) -> Result<(), DecodeError> {
        // `unsafe` block required in the next version of munge
        #[allow(unused_unsafe)]
        // SAFETY: `slot` is a valid `Slot` of `Envelope`. Destructuring it is safe.
        let encoded = unsafe {
            munge!(let Self { encoded } = slot);
            encoded
        };
        munge! {
            let Encoded {
                maybe_num_bytes,
                num_handles,
                flags,
            } = encoded;
        }

        if let Some(count) = Self::out_of_line_chunks(maybe_num_bytes, flags)? {
            return Err(DecodeError::ExpectedInline(count * CHUNK_SIZE));
        }

        decoder.__internal_take_handles(**num_handles as usize)?;

        Ok(())
    }

    /// Decodes and discards an unknown value in an envelope.
    #[inline]
    pub fn decode_unknown<'de, D: Decoder<'de> + ?Sized>(
        slot: Slot<'_, Self>,
        decoder: &mut D,
    ) -> Result<(), DecodeError> {
        // `unsafe` block required in the next version of munge
        #[allow(unused_unsafe)]
        // SAFETY: `slot` is a valid `Slot` of `Envelope`. Destructuring it is safe.
        let encoded = unsafe {
            munge!(let Self { encoded } = slot);
            encoded
        };
        munge! {
            let Encoded {
                maybe_num_bytes,
                num_handles,
                flags,
            } = encoded;
        }

        if let Some(count) = Self::out_of_line_chunks(maybe_num_bytes, flags)? {
            decoder.take_chunks(count)?;
        }

        decoder.__internal_take_handles(**num_handles as usize)?;

        Ok(())
    }

    /// Decodes a value of a known type from an envelope.
    #[inline]
    pub fn decode_as_static<D: InternalHandleDecoder + ?Sized, T: Decode<D>>(
        mut slot: Slot<'_, Self>,
        decoder: &mut D,
        constraint: T::Constraint,
    ) -> Result<(), DecodeError> {
        // `unsafe` block required in the next version of munge
        #[allow(unused_unsafe)]
        // SAFETY: `slot` is a valid `Slot` of `Envelope`. Destructuring it is safe.
        let encoded = unsafe {
            munge!(let Self { encoded } = slot.as_mut());
            encoded
        };
        munge! {
            let Encoded {
                maybe_num_bytes,
                num_handles,
                flags,
            } = encoded;
        }

        let handles_before = decoder.__internal_handles_remaining();
        let num_handles = **num_handles as usize;

        if let Some(count) = Self::out_of_line_chunks(maybe_num_bytes, flags)? {
            return Err(DecodeError::ExpectedInline(count * CHUNK_SIZE));
        }

        // Decode inline value
        if size_of::<T>() > INLINE_SIZE {
            return Err(DecodeError::InlineValueTooBig(size_of::<T>()));
        }
        // `unsafe` block required in the next version of munge
        #[allow(unused_unsafe)]
        // SAFETY: `slot` is a valid `Slot` of `Envelope`. Destructuring it is safe.
        let mut decoded_inline = unsafe {
            munge!(let Self { decoded_inline } = slot);
            decoded_inline
        };
        // SAFETY: `decoded_inline` is a slot for `[MaybeUninit<u8>; 4]` inside `Envelope`.
        // We cast its pointer to `*mut T`. Since `size_of::<T>() <= 4` and `Envelope` is aligned
        // to 8, the pointer is valid and aligned for `T`.
        let mut slot = unsafe { Slot::<T>::new_unchecked(decoded_inline.as_mut_ptr().cast()) };
        T::decode(slot.as_mut(), decoder, constraint)?;

        let handles_consumed = handles_before - decoder.__internal_handles_remaining();
        if handles_consumed != num_handles {
            return Err(DecodeError::IncorrectNumberOfHandlesConsumed {
                expected: num_handles,
                actual: handles_consumed,
            });
        }

        Ok(())
    }

    /// Decodes a value of a known type from an envelope.
    #[inline]
    pub fn decode_as<'de, D: Decoder<'de> + ?Sized, T: Decode<D>>(
        mut slot: Slot<'_, Self>,
        decoder: &mut D,
        constraint: T::Constraint,
    ) -> Result<(), DecodeError> {
        // `unsafe` block required in the next version of munge
        #[allow(unused_unsafe)]
        // SAFETY: `slot` is a valid `Slot` of `Envelope`. Destructuring it is safe.
        let encoded = unsafe {
            munge!(let Self { encoded } = slot.as_mut());
            encoded
        };
        munge! {
            let Encoded {
                mut maybe_num_bytes,
                num_handles,
                flags,
            } = encoded;
        }

        let handles_before = decoder.__internal_handles_remaining();
        let num_handles = **num_handles as usize;

        let out_of_line_chunks = Self::out_of_line_chunks(maybe_num_bytes.as_mut(), flags)?;
        if let Some(_count) = out_of_line_chunks {
            // Decode out-of-line value
            // TODO: set cap on decoder to make sure that the envelope doesn't decode more bytes
            // than it claims that it will
            let mut value_slot = decoder.take_slot::<T>()?;
            let value_ptr = value_slot.as_mut_ptr();
            T::decode(value_slot, decoder, constraint)?;

            // `unsafe` block required in the next version of munge
            #[allow(unused_unsafe)]
            // SAFETY: `slot` is a valid `Slot` of `Envelope`. Destructuring it is safe.
            let mut decoded_out_of_line = unsafe {
                munge!(let Self { decoded_out_of_line } = slot);
                decoded_out_of_line
            };
            // SAFETY: Identical to `ptr.write(value_ptr.cast())`, but raw
            // pointers don't currently implement `IntoBytes`.
            unsafe { decoded_out_of_line.as_mut_ptr().write(value_ptr.cast()) };
        } else {
            // Decode inline value
            if size_of::<T>() > INLINE_SIZE {
                return Err(DecodeError::InlineValueTooBig(size_of::<T>()));
            }
            // `unsafe` block required in the next version of munge
            #[allow(unused_unsafe)]
            // SAFETY: `slot` is a valid `Slot` of `Envelope`. Destructuring it is safe.
            let mut decoded_inline = unsafe {
                munge!(let Self { decoded_inline } = slot);
                decoded_inline
            };
            // SAFETY: `decoded_inline` is a slot for `[MaybeUninit<u8>; 4]` inside `Envelope`. We
            // cast its pointer to `*mut T`. Since `size_of::<T>() <= 4` and `Envelope` is aligned
            // to 8, the pointer is valid and aligned for `T`.
            let mut slot = unsafe { Slot::<T>::new_unchecked(decoded_inline.as_mut_ptr().cast()) };
            T::decode(slot.as_mut(), decoder, constraint)?;
        }

        let handles_consumed = handles_before - decoder.__internal_handles_remaining();
        if handles_consumed != num_handles {
            return Err(DecodeError::IncorrectNumberOfHandlesConsumed {
                expected: num_handles,
                actual: handles_consumed,
            });
        }

        Ok(())
    }

    /// Returns a pointer to the value contained in the envelope.
    ///
    /// # Safety
    ///
    /// `this` must point to a valid envelope that was successfully decoded.
    #[inline]
    pub unsafe fn as_ptr<T>(this: *mut Self) -> *mut T {
        if size_of::<T>() <= INLINE_SIZE {
            // SAFETY: `this` is valid and aligned as guaranteed by the caller.
            let inline = unsafe { addr_of_mut!((*this).decoded_inline) };
            inline.cast()
        } else {
            // SAFETY: `this` is valid and aligned as guaranteed by the caller, and contains
            // a decoded out-of-line pointer.
            unsafe { (*this).decoded_out_of_line.cast() }
        }
    }

    /// Returns a reference to the contained `T`.
    ///
    /// # Safety
    ///
    /// The envelope must have been successfully decoded as a `T`.
    #[inline]
    pub unsafe fn deref_unchecked<T>(&self) -> &T {
        // SAFETY: `self` is a valid reference, so we can cast it to a raw pointer.
        // `Self::as_ptr` is safe to call because `self` is valid and successfully decoded as `T`
        // (guaranteed by caller).
        let ptr = unsafe { Self::as_ptr::<T>((self as *const Self).cast_mut()).cast_const() };
        // SAFETY: `ptr` is valid and points to a decoded `T` (guaranteed by caller).
        unsafe { &*ptr }
    }

    /// Returns the contained `T`.
    ///
    /// # Safety
    ///
    /// The envelope must have been successfully decoded as a `T`. Reading from
    /// an envelope can cause undefined behavior if the underlying value is
    /// dropped later. Precautions should be taken to ensure that values read
    /// from an envelope are not dropped twice.
    #[inline]
    pub unsafe fn read_unchecked<T>(&self) -> T {
        // SAFETY: `into_raw(this)` is guaranteed to return a pointer that is non-null, properly
        // properly aligned, and valid for reads and writes.
        unsafe { Self::as_ptr::<T>((self as *const Self).cast_mut()).read() }
    }

    /// Takes the contained `T` out of the envelope.
    ///
    /// # Safety
    ///
    /// The envelope must have been successfully decoded as a `T`.
    #[inline]
    pub unsafe fn take_unchecked<T>(&mut self) -> T {
        // SAFETY: The caller guaranteed that the envelope was successfully
        // decoded as a `T`. The envelope is zeroed afterward to prevent double
        // drops.
        let result = unsafe { self.read_unchecked() };
        *self = Self::zero();
        result
    }

    /// Clones the envelope, assuming that it contains an inline `T`.
    ///
    /// # Safety
    ///
    /// The envelope must have been successfully decoded inline as a `T`.
    #[inline]
    pub unsafe fn clone_inline_unchecked<T: Clone>(&self) -> Self {
        debug_assert!(size_of::<T>() <= INLINE_SIZE);

        union ClonedToDecodedInline<T> {
            cloned: ManuallyDrop<T>,
            decoded_inline: [MaybeUninit<u8>; INLINE_SIZE],
        }

        // SAFETY: The caller guarantees that the envelope contains a decoded inline `T`.
        let cloned = unsafe { self.deref_unchecked::<T>().clone() };
        // SAFETY: Creating `Envelope` with `decoded_inline` field is safe.
        // `ClonedToDecodedInline` union is used to safely transmute `T` to `[MaybeUninit<u8>; 4]`.
        unsafe {
            Self {
                decoded_inline: ClonedToDecodedInline { cloned: ManuallyDrop::new(cloned) }
                    .decoded_inline,
            }
        }
    }
}
