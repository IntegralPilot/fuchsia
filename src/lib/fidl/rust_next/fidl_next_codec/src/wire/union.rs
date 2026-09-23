// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::MaybeUninit;
use core::ptr::addr_of_mut;

use munge::munge;

use crate::decoder::InternalHandleDecoder;
use crate::encoder::InternalHandleEncoder;
use crate::{
    Constrained, Decode, DecodeError, Decoder, Encode, EncodeError, Encoder, Slot, ValidationError,
    Wire, wire,
};

/// A raw FIDL union
#[repr(C)]
pub struct Union {
    ordinal: wire::Uint64,
    envelope: wire::Envelope,
}

impl Constrained for Union {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

// SAFETY: `Union` has stable layout (ordinal followed by envelope) and no padding.
unsafe impl Wire for Union {
    type Narrowed<'de> = Self;

    #[inline]
    fn zero_padding(_: &mut MaybeUninit<Self>) {
        // Wire unions have no padding
    }
}

impl Union {
    /// Encodes that a union is absent in a slot.
    #[inline]
    pub fn encode_absent(out: &mut MaybeUninit<Self>) {
        munge!(let Self { ordinal, envelope } = out);

        ordinal.write(wire::Uint64(0));
        wire::Envelope::encode_zero(envelope);
    }

    /// Encodes a `'static` value and ordinal in a slot.
    #[inline]
    pub fn encode_as_static<E: InternalHandleEncoder + ?Sized, W: Wire>(
        value: impl Encode<W, E>,
        ord: u64,
        encoder: &mut E,
        out: &mut MaybeUninit<Self>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        munge!(let Self { ordinal, envelope } = out);

        ordinal.write(wire::Uint64(ord));
        wire::Envelope::encode_value_static(value, encoder, envelope, constraint)
    }

    /// Encodes a value and ordinal in a slot.
    #[inline]
    pub fn encode_as<E: Encoder + ?Sized, W: Wire>(
        value: impl Encode<W, E>,
        ord: u64,
        encoder: &mut E,
        out: &mut MaybeUninit<Self>,
        constraint: W::Constraint,
    ) -> Result<(), EncodeError> {
        munge!(let Self { ordinal, envelope } = out);

        ordinal.write(wire::Uint64(ord));
        wire::Envelope::encode_value(value, encoder, envelope, constraint)
    }

    /// Returns the ordinal of the encoded value.
    #[inline]
    pub fn encoded_ordinal(slot: Slot<'_, Self>) -> u64 {
        munge!(let Self { ordinal, envelope: _ } = slot);
        **ordinal
    }

    /// Decodes an absent union from a slot.
    #[inline]
    pub fn decode_absent(slot: Slot<'_, Self>) -> Result<(), DecodeError> {
        munge!(let Self { ordinal: _, envelope } = slot);
        if !wire::Envelope::is_encoded_zero(envelope) {
            return Err(DecodeError::InvalidUnionEnvelope);
        }
        Ok(())
    }

    /// Decodes an unknown `'static` value from a union.
    ///
    /// The handles owned by the unknown value are discarded.
    #[inline]
    pub fn decode_unknown_static<D: InternalHandleDecoder + ?Sized>(
        slot: Slot<'_, Self>,
        decoder: &mut D,
    ) -> Result<(), DecodeError> {
        munge!(let Self { ordinal: _, envelope } = slot);
        wire::Envelope::decode_unknown_static(envelope, decoder)
    }

    /// Decodes an unknown value from a union.
    ///
    /// The handles owned by the unknown value are discarded.
    #[inline]
    pub fn decode_unknown<'de, D: Decoder<'de> + ?Sized>(
        slot: Slot<'_, Self>,
        decoder: &mut D,
    ) -> Result<(), DecodeError> {
        munge!(let Self { ordinal: _, envelope } = slot);
        wire::Envelope::decode_unknown(envelope, decoder)
    }

    /// Decodes the typed `'static` value in a union.
    #[inline]
    pub fn decode_as_static<D: InternalHandleDecoder + ?Sized, T: Decode<D>>(
        slot: Slot<'_, Self>,
        decoder: &mut D,
        constraint: T::Constraint,
    ) -> Result<(), DecodeError> {
        munge!(let Self { ordinal: _, envelope } = slot);
        wire::Envelope::decode_as_static::<D, T>(envelope, decoder, constraint)
    }

    /// Decodes the typed value in a union.
    #[inline]
    pub fn decode_as<'de, D: Decoder<'de> + ?Sized, T: Decode<D>>(
        slot: Slot<'_, Self>,
        decoder: &mut D,
        constraint: T::Constraint,
    ) -> Result<(), DecodeError> {
        munge!(let Self { ordinal: _, envelope } = slot);
        wire::Envelope::decode_as::<D, T>(envelope, decoder, constraint)
    }

    /// The absent optional union.
    #[inline]
    pub fn absent() -> Self {
        Self { ordinal: wire::Uint64(0), envelope: wire::Envelope::zero() }
    }

    /// Returns whether the union contains a value.
    #[inline]
    pub fn is_some(&self) -> bool {
        *self.ordinal != 0
    }

    /// Returns whether the union is empty.
    #[inline]
    pub fn is_none(&self) -> bool {
        !self.is_some()
    }

    /// Returns the ordinal of the union.
    #[inline]
    pub fn ordinal(&self) -> u64 {
        *self.ordinal
    }

    /// Gets a raw pointer to the envelope underlying the union.
    ///
    /// # Safety
    ///
    /// `this` must be non-null, properly aligned, and valid for reads.
    #[inline]
    pub unsafe fn get_raw(this: *mut Self) -> *mut wire::Envelope {
        // SAFETY: `this` is valid and aligned as guaranteed by the caller.
        unsafe { addr_of_mut!((*this).envelope) }
    }

    /// Gets a reference to the envelope underlying the union.
    #[inline]
    pub fn get(&self) -> &wire::Envelope {
        &self.envelope
    }

    /// Clones the union, assuming that it contains an inline `T`.
    ///
    /// # Safety
    ///
    /// The union must have been successfully decoded inline as a `T`.
    #[inline]
    pub unsafe fn clone_inline_unchecked<T: Clone>(&self) -> Self {
        Self {
            ordinal: self.ordinal,
            // SAFETY: The caller guarantees that the union contains a decoded inline `T`,
            // which satisfies the precondition of `clone_inline_unchecked`.
            envelope: unsafe { self.envelope.clone_inline_unchecked::<T>() },
        }
    }
}
