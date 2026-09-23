// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::MaybeUninit;

use munge::munge;

use crate::{
    Constrained, DecodeError, Decoder, DecoderExt as _, Slot, ValidationError, Wire, wire,
};

/// A FIDL table
#[repr(C)]
pub struct Table<'de> {
    len: wire::Uint64,
    ptr: wire::Pointer<'de, wire::Envelope>,
}

impl Constrained for Table<'_> {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

// SAFETY: `Table` has stable layout and no padding.
unsafe impl Wire for Table<'static> {
    type Narrowed<'de> = Table<'de>;

    #[inline]
    fn zero_padding(_: &mut MaybeUninit<Self>) {
        // Wire tables have no padding
    }
}

impl<'de> Table<'de> {
    /// Encodes that a table contains `len` values in a slot.
    #[inline]
    pub fn encode_len(out: &mut MaybeUninit<Self>, len: usize) {
        munge!(let Self { len: table_len, ptr } = out);
        table_len.write(wire::Uint64(len.try_into().unwrap()));
        wire::Pointer::encode_present(ptr);
    }

    /// Decodes the fields of the table with a decoding function.
    ///
    /// The decoding function receives the ordinal of the field, its slot, and the decoder.
    #[inline]
    pub fn decode_with<D: Decoder<'de> + ?Sized>(
        slot: Slot<'_, Self>,
        decoder: &mut D,
        f: impl Fn(i64, Slot<'_, wire::Envelope>, &mut D) -> Result<(), DecodeError>,
    ) -> Result<(), DecodeError> {
        munge!(let Self { len, mut ptr } = slot);

        if wire::Pointer::is_encoded_present(ptr.as_mut())? {
            let mut envelopes = decoder.take_slice_slot::<wire::Envelope>(**len as usize)?;

            for i in 0..**len as usize {
                let mut envelope = envelopes.index(i);
                if !wire::Envelope::is_encoded_zero(envelope.as_mut()) {
                    f((i + 1) as i64, envelope, decoder)?;
                }
            }

            wire::Pointer::set_decoded_slice(ptr, envelopes);
        } else if **len != 0 {
            return Err(DecodeError::InvalidOptionalSize(**len));
        }

        Ok(())
    }
}

impl Table<'_> {
    #[inline]
    fn get_raw(&self, ordinal: usize) -> Option<*mut wire::Envelope> {
        if ordinal == 0 || ordinal > *self.len as usize {
            return None;
        }

        // SAFETY: `self.ptr` points to an array of `Envelope` of length `self.len`.
        // We checked that `ordinal` is within `1..=self.len`, so `ordinal - 1` is within bounds.
        unsafe {
            let ptr = self.ptr.as_ptr().add(ordinal - 1);
            (!(*ptr).is_zero()).then_some(ptr)
        }
    }

    /// Returns a reference to the envelope for the given ordinal, if any.
    #[inline]
    pub fn get(&self, ordinal: usize) -> Option<&wire::Envelope> {
        // SAFETY: `get_raw` always returns an envelope that is safe to
        // dereference. We have a reference to `self`, so we can also alias
        // the envelope.
        unsafe { Some(&*self.get_raw(ordinal)?) }
    }

    /// Returns a mutable reference to the envelope for the given ordinal, if any.
    #[inline]
    pub fn get_mut(&mut self, ordinal: usize) -> Option<&mut wire::Envelope> {
        // SAFETY: `get_raw` always returns an envelope that is safe to
        // dereference. We have a mutable reference to `self`, so we can also
        // mutably alias the envelope.
        unsafe { Some(&mut *self.get_raw(ordinal)?) }
    }
}
