// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;
use core::mem::MaybeUninit;
use core::ops::Deref;
use core::str::{from_utf8, from_utf8_unchecked};

use munge::munge;

use crate::{
    Constrained, Decode, DecodeError, Decoder, Encode, EncodeError, Encoder, FromWire, FromWireRef,
    IntoNatural, Slot, ValidationError, Wire, wire,
};

use std::string::String as StdString;

/// A FIDL string
#[repr(transparent)]
pub struct String<'de> {
    vec: wire::Vector<'de, u8>,
}

// SAFETY: `String` is a `#[repr(transparent)]` wrapper around `wire::Vector<'static, u8>`, which
// is `Wire`.
unsafe impl Wire for String<'static> {
    type Narrowed<'de> = String<'de>;

    #[inline]
    fn zero_padding(out: &mut MaybeUninit<Self>) {
        munge!(let Self { vec } = out);
        wire::Vector::<u8>::zero_padding(vec);
    }
}

impl String<'_> {
    /// Encodes that a string is present in a slot.
    #[inline]
    pub fn encode_present(out: &mut MaybeUninit<Self>, len: u64) {
        munge!(let Self { vec } = out);
        wire::Vector::encode_present(vec, len);
    }

    /// Returns the length of the string in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.vec.len()
    }

    /// Returns whether the string is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns a reference to the underlying `str`.
    #[inline]
    pub fn as_str(&self) -> &str {
        // SAFETY: The bytes of a decoded `String` are validated to be valid UTF-8.
        unsafe { from_utf8_unchecked(self.vec.as_slice()) }
    }

    /// Validate that this string's length falls within the limit.
    fn validate_max_len(slot: Slot<'_, Self>, limit: u64) -> Result<(), ValidationError> {
        munge!(let Self { vec } = slot);
        match wire::Vector::validate_max_len(vec, limit) {
            Ok(()) => Ok(()),
            Err(ValidationError::VectorTooLong { count, limit }) => {
                Err(ValidationError::StringTooLong { count, limit })
            }
            Err(e) => Err(e),
        }
    }
}

impl Constrained for String<'_> {
    type Constraint = u64;

    fn validate(slot: Slot<'_, Self>, constraint: u64) -> Result<(), ValidationError> {
        Self::validate_max_len(slot, constraint)
    }
}

impl Deref for String<'_> {
    type Target = str;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl fmt::Debug for String<'_> {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl<U: ?Sized> PartialEq<&U> for String<'_>
where
    for<'de> String<'de>: PartialEq<U>,
{
    fn eq(&self, other: &&U) -> bool {
        self == *other
    }
}

impl PartialEq for String<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<str> for String<'_> {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

// SAFETY: If `decode` returns `Ok`, `slot` is guaranteed to contain a valid decoded `String`
// because it delegates to `wire::Vector::decode_raw` and validates that the decoded bytes
// are valid UTF-8.
unsafe impl<'de, D: Decoder<'de> + ?Sized> Decode<D> for String<'de> {
    #[inline]
    fn decode(slot: Slot<'_, Self>, decoder: &mut D, constraint: u64) -> Result<(), DecodeError> {
        munge!(let Self { mut vec } = slot);

        // SAFETY: `vec` is a valid slot for `Vector`.
        match unsafe { wire::Vector::decode_raw(vec.as_mut(), decoder, constraint) } {
            Ok(()) => (),
            Err(DecodeError::Validation(ValidationError::VectorTooLong { count, limit })) => {
                return Err(DecodeError::Validation(ValidationError::StringTooLong {
                    count,
                    limit,
                }));
            }
            Err(e) => {
                return Err(e);
            }
        };
        // SAFETY: `decode_raw` succeeded, so the slot contents are valid.
        let vec = unsafe { vec.deref_unchecked() };

        // Check if the string is valid ASCII (fast path)
        if !vec.as_slice().is_ascii() {
            // Fall back to checking if the string is valid UTF-8 (slow path)
            // We're using `from_utf8` more like an `is_utf8` here.
            let _ = from_utf8(vec.as_slice())?;
        }

        Ok(())
    }
}

// SAFETY: Delegates to `<&str>::encode` which is safe and fully initializes the output.
unsafe impl<E: Encoder + ?Sized> Encode<String<'static>, E> for StdString {
    #[inline]
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<String<'static>>,
        constraint: u64,
    ) -> Result<(), EncodeError> {
        self.as_str().encode(encoder, out, constraint)
    }
}

// SAFETY: Delegates to `<&str>::encode` which is safe and fully initializes the output.
unsafe impl<E: Encoder + ?Sized> Encode<String<'static>, E> for &StdString {
    #[inline]
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<String<'static>>,
        constraint: u64,
    ) -> Result<(), EncodeError> {
        self.as_str().encode(encoder, out, constraint)
    }
}

// SAFETY: `String` has no padding. `encode` initializes the output fully by writing
// the string bytes to the encoder and calling `String::encode_present`.
unsafe impl<E: Encoder + ?Sized> Encode<String<'static>, E> for &str {
    #[inline]
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<String<'static>>,
        _constraint: u64,
    ) -> Result<(), EncodeError> {
        encoder.write(self.as_bytes());
        String::encode_present(out, self.len() as u64);
        Ok(())
    }
}

impl FromWire<String<'_>> for StdString {
    #[inline]
    fn from_wire(wire: String<'_>) -> Self {
        StdString::from_wire_ref(&wire)
    }
}

impl IntoNatural for String<'_> {
    type Natural = StdString;
}

impl FromWireRef<String<'_>> for StdString {
    #[inline]
    fn from_wire_ref(wire: &String<'_>) -> Self {
        // SAFETY: The bytes of a decoded `String` are validated to be valid UTF-8.
        unsafe { StdString::from_utf8_unchecked(Vec::from_wire_ref(&wire.vec)) }
    }
}

#[cfg(test)]
mod tests {
    use crate::{DecoderExt as _, EncoderExt as _, chunks, wire};

    #[test]
    fn decode_string() {
        assert_eq!(
            chunks![
                0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0x30, 0x31, 0x32, 0x33, 0x00, 0x00, 0x00, 0x00,
            ]
            .as_mut_slice()
            .decode_with_constraint::<wire::String<'_>>(1000)
            .unwrap(),
            "0123",
        );
        assert_eq!(
            chunks![
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff,
            ]
            .as_mut_slice()
            .decode_with_constraint::<wire::String<'_>>(1000)
            .unwrap(),
            "",
        );
    }

    #[test]
    fn encode_string() {
        assert_eq!(
            Vec::encode_with_constraint(Some("0123".to_string()), 1000).unwrap(),
            chunks![
                0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0x30, 0x31, 0x32, 0x33, 0x00, 0x00, 0x00, 0x00,
            ],
        );
        assert_eq!(
            Vec::encode_with_constraint(Some(String::new()), 1000).unwrap(),
            chunks![
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff,
            ],
        );
    }
}
