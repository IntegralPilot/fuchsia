// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;

/// A decodable type can be created from a byte buffer.
/// The type returned is separate (copied) from the buffer once decoded.
pub trait Decodable: ::core::marker::Sized {
    type Error;

    /// Decodes into a new object or an error, and the number of bytes that
    /// the decoding consumed.  Should attempt to consume the entire item from
    /// the buffer in the case of an error.  If the item end cannot be
    /// determined, return an error and consume the entirety of the bufer
    /// (`buf.len()`)
    fn decode(buf: &[u8]) -> (::core::result::Result<Self, Self::Error>, usize);

    /// Tries to decode a collection of this object concatenated in a buffer.
    /// Returns a vector of items (or errors) and the number of bytes consumed
    /// to decode them.
    /// Continues to decode items until the buffer is consumed or the max items.
    /// If None, will decode the entire buffer.
    fn decode_multiple(
        buf: &[u8],
        max: Option<usize>,
    ) -> (Vec<::core::result::Result<Self, Self::Error>>, usize) {
        let mut idx = 0;
        let mut result = Vec::new();
        while idx < buf.len() && Some(result.len()) != max {
            let (one_result, consumed) = Self::decode(&buf[idx..]);
            result.push(one_result);
            idx += consumed;
        }
        (result, idx)
    }
}

/// A decodable type that has an exact size in bytes.
pub trait FixedSizeDecodable: ::core::marker::Sized {
    type Error;
    const WRONG_SIZE_ERROR: Self::Error;
    const BYTE_SIZE: usize;

    /// Decodes the value.  This function assumes that buf is of at least
    /// BYTE_SIZE, and assumes that BYTE_SIZE bytes are consumed to decode.
    fn decode_checked(buf: &[u8]) -> core::result::Result<Self, Self::Error>;
}

/// An encodable type can write itself into a byte buffer.
pub trait Encodable {
    type Error;

    /// Returns the number of bytes necessary to encode |self|.
    fn encoded_len(&self) -> ::core::primitive::usize;

    /// Writes the encoded version of |self| at the start of |buf|.
    /// |buf| must be at least |self.encoded_len()| length.
    fn encode(&self, buf: &mut [u8]) -> ::core::result::Result<(), Self::Error>;
}

/// Generates an enum value where each variant can be converted into a constant
/// in the given raw_type.
///
/// For example:
/// decodable_enum! {
///     pub(crate) enum Color<u8, MyError, Variant> {
///        Red = 1,
///        Blue = 2,
///        Green = 3,
///     }
/// }
///
/// Color::try_from(2) -> Color::Red
/// u8::from(&Color::Red) -> 1.
#[macro_export]
macro_rules! decodable_enum {
    ($(#[$meta:meta])* $visibility:vis enum $name:ident<
        $raw_type:ty,
        $error_type:ty,
        $error_path:ident
    > {
        $($(#[$variant_meta:meta])* $variant:ident = $val:expr),*,
    }) => {
        $(#[$meta])*
        #[derive(
            ::core::clone::Clone,
            ::core::marker::Copy,
            ::core::fmt::Debug,
            ::core::cmp::Eq,
            ::core::hash::Hash,
            ::core::cmp::PartialEq)]
        $visibility enum $name {
            $($(#[$variant_meta])* $variant = $val),*
        }

        impl $name {
            pub const VALUES : &'static [$raw_type] = &[$($val),*,];
            pub const VARIANTS : &'static [$name] = &[$($name::$variant),*,];
            pub fn name(&self) -> &'static ::core::primitive::str {
                match self {
                    $($name::$variant => ::core::stringify!($variant)),*
                }
            }
        }

        impl ::core::convert::From<$name> for $raw_type {
            fn from(v: $name) -> $raw_type {
                match v {
                    $($name::$variant => $val),*,
                }
            }
        }

        impl ::core::convert::TryFrom<$raw_type> for $name {
            type Error = $error_type;

            fn try_from(value: $raw_type) -> ::core::result::Result<Self, $error_type> {
                match value {
                    $($val => ::core::result::Result::Ok($name::$variant)),*,
                    _ => ::core::result::Result::Err(<$error_type>::$error_path),
                }
            }
        }
    }
}

#[macro_export]
macro_rules! codable_as_bitmask {
    ($type:ty, $raw_type:ty) => {
        impl $type {
            pub fn from_bits(v: $raw_type) -> impl Iterator<Item = $type> {
                (0..<$raw_type>::BITS)
                    .map(|bit| 1 << bit)
                    .filter(move |val| (v & val) != 0)
                    .filter_map(|val| val.try_into().ok())
            }

            pub fn to_bits<'a>(it: impl Iterator<Item = &'a $type>) -> $raw_type {
                it.fold(0, |acc, item| acc | Into::<$raw_type>::into(*item))
            }
        }
    };
}

#[derive(Error, Debug, PartialEq)]
pub enum Error {
    #[error("Parameter is not valid: {0}")]
    InvalidParameter(String),

    #[error("Out-of-range enum value")]
    OutOfRange,

    #[error("Encoding buffer is too small")]
    BufferTooSmall,

    #[error("Buffer being decoded is invalid length")]
    UnexpectedDataLength,

    #[error("Unrecognized type for {0}: {1}")]
    UnrecognizedType(String, u8),

    #[error("Uuid parsing error: {0}")]
    Uuid(uuid::Error),
}
