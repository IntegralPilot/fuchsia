// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Generic utilities for encoding/decoding packets.

/// Generates an enum value where each variant can be converted into a constant in the given
/// raw_type.
///
/// For example:
/// decodable_enum! {
///     pub(crate) enum Color<u8, MyError, MyError::Variant> {
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
        $error_type:ident,
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

        impl ::core::convert::From<&$name> for $raw_type {
            fn from(v: &$name) -> $raw_type {
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
                    _ => ::core::result::Result::Err($error_type::$error_path),
                }
            }
        }
    }
}

/// A decodable type can be created from a byte buffer.
/// The type returned is separate (copied) from the buffer once decoded.
pub trait Decodable: ::core::marker::Sized {
    type Error;

    /// Decodes into a new object, or returns an error.
    fn decode(buf: &[u8]) -> ::core::result::Result<Self, Self::Error>;
}

/// An encodable type can write itself into a byte buffer.
pub trait Encodable: ::core::marker::Sized {
    type Error;

    /// Returns the number of bytes necessary to encode |self|.
    fn encoded_len(&self) -> ::core::primitive::usize;
    /// Writes the encoded version of |self| at the start of |buf|.
    /// |buf| must be at least |self.encoded_len()| length.
    fn encode(&self, buf: &mut [u8]) -> ::core::result::Result<(), Self::Error>;
}

#[macro_export]
macro_rules! codable_as_bitmask {
    ($type:ty, $raw_type:ty, $error_type:ident, $error_path:ident) => {
        impl $type {
            pub fn from_bits(
                v: $raw_type,
            ) -> impl ::std::iter::Iterator<Item = ::core::result::Result<$type, $error_type>> {
                (0..<$raw_type>::BITS).map(|bit| 1 << bit).filter(move |val| (v & val) != 0).map(
                    |val| {
                        ::std::convert::TryInto::<$type>::try_into(val)
                            .map_err(|_| $error_type::$error_path)
                    },
                )
            }

            pub fn to_bits<'a>(
                mut it: impl ::std::iter::Iterator<Item = &'a $type>,
            ) -> ::core::result::Result<$raw_type, $error_type> {
                it.try_fold(0, |acc, item| {
                    let v = ::std::convert::Into::<$raw_type>::into(item);
                    if v == 0 {
                        return ::core::result::Result::Err($error_type::$error_path);
                    }
                    if (v as f64).log2().ceil() != (v as f64).log2().floor() {
                        return ::core::result::Result::Err($error_type::$error_path);
                    }
                    ::core::result::Result::Ok(acc | v)
                })
            }
        }
    };
}

#[cfg(test)]
#[no_implicit_prelude]
mod test {
    use ::assert_matches::assert_matches;
    use ::core::convert::{From, TryFrom};
    use ::core::option::Option::Some;
    use ::core::result::Result;
    use ::core::{assert, assert_eq, panic};
    use ::std::collections::HashSet;
    use ::std::iter::{IntoIterator, Iterator};
    use ::std::vec;

    #[derive(Debug, PartialEq)]
    pub(crate) enum TestError {
        OutOfRange,
    }

    decodable_enum! {
        pub(crate) enum TestEnum<u16, TestError, OutOfRange> {
            One = 0x0001,
            Two = 0x0002,
            Max = 0xFFFF,
        }
    }
    codable_as_bitmask!(TestEnum, u16, TestError, OutOfRange);

    decodable_enum! {
        pub(crate) enum TestEnum2<u16, TestError, OutOfRange> {
            One = 0x0001,  // bit 0
            Two = 0x0002,  // bit 1
            Big = 0x4000,  // bit 14
        }
    }
    codable_as_bitmask!(TestEnum2, u16, TestError, OutOfRange);

    #[test]
    fn try_from_success() {
        let one = TestEnum::try_from(1);
        assert!(one.is_ok());
        assert_eq!(TestEnum::One, one.unwrap());
        let two = TestEnum::try_from(2);
        assert!(two.is_ok());
        assert_eq!(TestEnum::Two, two.unwrap());
        let max = TestEnum::try_from(65535);
        assert!(max.is_ok());
        assert_eq!(TestEnum::Max, max.unwrap());
    }

    #[test]
    fn try_from_error() {
        let err = TestEnum::try_from(5);
        assert_matches!(err.err(), Some(TestError::OutOfRange));
    }

    #[test]
    fn into_rawtype() {
        let raw = u16::from(&TestEnum::One);
        assert_eq!(1, raw);
        let raw = u16::from(&TestEnum::Two);
        assert_eq!(2, raw);
        let raw = u16::from(&TestEnum::Max);
        assert_eq!(65535, raw);
    }

    #[test]
    fn test_values() {
        let v = TestEnum::VALUES.to_vec();
        assert_eq!(3, v.len());
        assert_eq!(1, v[0]);
        assert_eq!(2, v[1]);
        assert_eq!(65535, v[2]);

        let v = TestEnum2::VALUES.to_vec();
        assert_eq!(v, vec![1, 2, 16384]);
    }

    #[test]
    fn test_variants() {
        let v = TestEnum::VARIANTS.to_vec();
        assert_eq!(3, v.len());
        assert_eq!(TestEnum::One, v[0]);
        assert_eq!(TestEnum::Two, v[1]);
        assert_eq!(TestEnum::Max, v[2]);
    }

    #[test]
    fn test_name() {
        assert_eq!("One", TestEnum::One.name());
        assert_eq!("Two", TestEnum::Two.name());
        assert_eq!("Max", TestEnum::Max.name());
        assert_eq!("Big", TestEnum2::Big.name());
    }

    #[test]
    fn as_bitmask() {
        let one_and_big = 0x4001;

        let enums: HashSet<TestEnum2> = TestEnum2::from_bits(one_and_big)
            .collect::<Result<HashSet<_>, _>>()
            .expect("should not fail");

        assert_eq!(2, enums.len());

        let expected_enums = [TestEnum2::One, TestEnum2::Big].into_iter().collect();

        assert_eq!(enums, expected_enums);

        let all = TestEnum2::VARIANTS;
        let value = TestEnum2::to_bits(all.iter()).expect("should work");
        assert_eq!(0x4003, value);
    }

    #[test]
    fn bitmask_errors() {
        // Max value has both bit one and two set which are valid TestEnum variants.
        // The rest of the set bits are not valid TestEnum variants.
        let max = 65535;
        // Collecting as result shows failure.
        let res: Result<HashSet<TestEnum>, _> = TestEnum::from_bits(max).collect();
        let _ = res.expect_err("should have failed");
        // Collecting as vector of results show only 2 "bit" values were valid enums.
        let res: vec::Vec<Result<TestEnum, TestError>> = TestEnum::from_bits(max).collect();
        let valid: vec::Vec<TestEnum> = res.into_iter().filter_map(|v| v.ok()).collect();
        assert_eq!(valid.len(), 2);

        // Fails because TestEnum::Max variant is not a bitwise flag value.
        let all = TestEnum::VARIANTS;
        let _ = TestEnum::to_bits(all.iter()).expect_err("should fail");
    }
}
