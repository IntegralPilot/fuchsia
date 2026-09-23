// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;
use core::mem::{ManuallyDrop, forget};
use core::ops::Deref;
use core::ptr::NonNull;

use crate::{Chunk, Decode, DecodeError, Decoder, DecoderExt as _, FromWire, IntoNatural, Wire};

/// A type that can be borrowed as a decoder.
///
/// # Safety
///
/// Moving a value of this type must not invalidate any references to chunks
/// returned from `take_chunks`. This usually means that the chunks returned
/// from `take_chunks` point to non-local memory (e.g. on the heap, elsewhere on
/// the stack).
pub unsafe trait AsDecoder<'de> {
    /// The borrowed decoder type.
    type Decoder: Decoder<'de>;

    /// Borrowes this value as a decoder.
    fn as_decoder(&'de mut self) -> Self::Decoder;
}

// SAFETY: `Vec` stores its elements on the heap, so moving the `Vec` does not invalidate
// references to its elements.
unsafe impl<'de> AsDecoder<'de> for Vec<Chunk> {
    type Decoder = &'de mut [Chunk];

    fn as_decoder(&'de mut self) -> Self::Decoder {
        self.as_mut_slice()
    }
}

/// Extension methods for `AsDecoder`.
pub trait AsDecoderExt: for<'de> AsDecoder<'de> {
    /// Decodes a value from the decoder and finishes it.
    ///
    /// On success, returns `Ok` of a `Decoded` value with the decoder. Returns `Err` if decoding
    /// failed or the decoder finished with an error.
    fn into_decoded<T>(self) -> Result<Decoded<T, Self>, DecodeError>
    where
        Self: for<'de> AsDecoder<'de> + Sized,
        T: Wire<Constraint = ()>,
        for<'de> T::Narrowed<'de>: Decode<<Self as AsDecoder<'de>>::Decoder, Constraint = ()>;

    /// Decodes a value from the decoder and finishes it.
    ///
    /// On success, returns `Ok` of a `Decoded` value with the decoder. Returns `Err` if decoding
    /// failed or the decoder finished with an error.
    fn into_decoded_with_constraint<T>(
        self,
        constraint: T::Constraint,
    ) -> Result<Decoded<T, Self>, DecodeError>
    where
        Self: for<'de> AsDecoder<'de> + Sized,
        T: Wire,
        for<'de> T::Narrowed<'de>:
            Decode<<Self as AsDecoder<'de>>::Decoder, Constraint = T::Constraint>;
}

impl<D: for<'de> AsDecoder<'de>> AsDecoderExt for D {
    fn into_decoded<T>(self) -> Result<Decoded<T, Self>, DecodeError>
    where
        Self: for<'de> AsDecoder<'de> + Sized,
        T: Wire<Constraint = ()>,
        for<'de> T::Narrowed<'de>: Decode<<Self as AsDecoder<'de>>::Decoder, Constraint = ()>,
    {
        Self::into_decoded_with_constraint::<T>(self, ())
    }

    fn into_decoded_with_constraint<T>(
        mut self,
        constraint: T::Constraint,
    ) -> Result<Decoded<T, Self>, DecodeError>
    where
        Self: for<'de> AsDecoder<'de> + Sized,
        T: Wire,
        for<'de> T::Narrowed<'de>:
            Decode<<Self as AsDecoder<'de>>::Decoder, Constraint = T::Constraint>,
    {
        let mut decoder = self.as_decoder();
        let mut slot = decoder.take_slot::<T::Narrowed<'_>>()?;
        T::Narrowed::decode(slot.as_mut(), &mut decoder, constraint)?;
        decoder.commit();
        decoder.finish()?;

        // SAFETY: The slot pointer is obtained from a valid slot in the decoder, which is
        // non-null.
        let ptr = unsafe { NonNull::new_unchecked(slot.as_mut_ptr().cast()) };
        drop(decoder);
        Ok(Decoded { ptr, decoder: ManuallyDrop::new(self) })
    }
}

/// A decoded value and the decoder which contains it.
pub struct Decoded<T: ?Sized, D> {
    ptr: NonNull<T>,
    decoder: ManuallyDrop<D>,
}

// SAFETY: `Decoded` doesn't add any restrictions on sending across thread boundaries, and so is
// `Send` if `T` and `D` are `Send`.
unsafe impl<T: Send + ?Sized, D: Send> Send for Decoded<T, D> {}

// SAFETY: `Decoded` doesn't add any interior mutability, so it is `Sync` if `T` and `D` are
// `Sync`.
unsafe impl<T: Sync + ?Sized, D: Sync> Sync for Decoded<T, D> {}

impl<T: ?Sized, D> Drop for Decoded<T, D> {
    fn drop(&mut self) {
        // SAFETY: `ptr` points to a `T` which is safe to drop as an invariant of `Decoded`. We
        // will only ever drop it once, since `drop` may only be called once.
        unsafe {
            self.ptr.as_ptr().drop_in_place();
        }
        // SAFETY: `decoder` is only ever dropped once, since `drop` may only be called once.
        unsafe {
            ManuallyDrop::drop(&mut self.decoder);
        }
    }
}

impl<T: ?Sized, D> Decoded<T, D> {
    /// Returns a new `Decoded` of the given pointer and decoder.
    ///
    /// # Safety
    ///
    /// `ptr` must point to a valid value and remain valid as long as `decoder`
    /// is not dropped.
    pub unsafe fn new_unchecked(ptr: *mut T, decoder: D) -> Self {
        // SAFETY: `ptr` is non-null as a precondition of `new_unchecked`.
        Self { ptr: unsafe { NonNull::new_unchecked(ptr) }, decoder: ManuallyDrop::new(decoder) }
    }

    /// Returns the raw pointer and decoder used to create this `Decoded`.
    pub fn into_raw_parts(mut self) -> (*mut T, D) {
        let ptr = self.ptr.as_ptr();
        // SAFETY: We forget `self` immediately after taking `self.decoder`, so we won't
        // double-drop the decoder.
        let decoder = unsafe { ManuallyDrop::take(&mut self.decoder) };
        forget(self);
        (ptr, decoder)
    }

    /// Takes the value out of this `Decoded` and calls `FromWire::from_wire` on the taken value to
    /// convert it to the default natural type.
    ///
    /// This consumes the `Decoded`.
    pub fn take(self) -> T::Natural
    where
        T: Wire + IntoNatural,
        T::Natural: for<'de> FromWire<T::Narrowed<'de>>,
    {
        self.take_as::<T::Natural>()
    }

    /// Takes the value out of this `Decoded` and calls `U::from_wire` on the taken value.
    ///
    /// This consumes the `Decoded`.
    pub fn take_as<U>(self) -> U
    where
        T: Wire,
        U: for<'de> FromWire<T::Narrowed<'de>>,
    {
        self.take_with(|wire| U::from_wire(wire))
    }

    /// Takes the value out of this `Decoded` and passes it to the given function.
    ///
    /// This consumes the `Decoded`.
    pub fn take_with<U>(self, f: impl for<'de> FnOnce(T::Narrowed<'de>) -> U) -> U
    where
        T: Wire,
    {
        let (ptr, decoder) = self.into_raw_parts();
        // SAFETY: `ptr` points to a valid value of type `T::Narrowed`, and ownership of the
        // value is transferred here.
        let value = unsafe { ptr.cast::<T::Narrowed<'_>>().read() };
        let result = f(value);
        drop(decoder);
        result
    }
}

impl<T: ?Sized, B> Deref for Decoded<T, B> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: `ptr` is non-null, properly-aligned, and valid for reads and writes as an
        // invariant of `Decoded`.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: fmt::Debug + ?Sized, B: fmt::Debug> fmt::Debug for Decoded<T, B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(f)
    }
}
