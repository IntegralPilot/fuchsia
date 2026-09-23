// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;

use crate::packet_encoding::{Decodable, Encodable};

/// Implement Ltv when a collection of types is represented in the Bluetooth
/// specifications as a length-type-value structure.  They should have an
/// associated type which can be retrieved from a type byte.
pub trait LtValue: Sized {
    type Type: Into<u8> + Copy + std::fmt::Debug;

    const NAME: &'static str;

    /// Given a type octet, return the associated Type if it is possible.
    /// Returns None if the value is unrecognized.
    fn type_from_octet(x: u8) -> Option<Self::Type>;

    /// Returns length bounds for the type indicated, **including** the type
    /// byte. Note that the assigned numbers from the Bluetooth SIG include
    /// the type byte in their Length specifications.
    // TODO: use impl std::ops::RangeBounds when RPITIT is sufficiently stable
    fn length_range_from_type(ty: Self::Type) -> std::ops::RangeInclusive<u8>;

    /// Retrieve the type of the current value.
    fn into_type(&self) -> Self::Type;

    /// The length of the encoded value, without the length and type byte.
    /// This cannot be 255 in practice, as the length byte is only one octet
    /// long.
    fn value_encoded_len(&self) -> u8;

    /// Decodes the value from a buffer, which does not include the type or
    /// length bytes. The `buf` slice length is exactly what was specified
    /// for this value in the encoded source.
    fn decode_value(ty: &Self::Type, buf: &[u8]) -> Result<Self, crate::packet_encoding::Error>;

    /// Encodes a value into `buf`, which is verified to be the correct length
    /// as indicated by [LtValue::value_encoded_len].
    fn encode_value(&self, buf: &mut [u8]) -> Result<(), crate::packet_encoding::Error>;

    /// Decode a collection of LtValue structures that are present in a buffer.
    /// If it is possible to continue decoding after encountering an error, does
    /// so and includes the error. If an unrecoverable error occurs, does
    /// not consume the final item and the last element in the result is the
    /// error.
    fn decode_all(buf: &[u8]) -> (Vec<Result<Self, Error<Self::Type>>>, usize) {
        let mut results = Vec::new();
        let mut total_consumed = 0;
        loop {
            if buf.len() <= total_consumed {
                return (results, std::cmp::min(buf.len(), total_consumed));
            }
            let indicated_len = buf[total_consumed] as usize;
            let range_end = std::cmp::min(buf.len() - 1, total_consumed + indicated_len);
            match Self::decode(&buf[total_consumed..=range_end]) {
                (Ok(item), consumed) => {
                    results.push(Ok(item));
                    total_consumed += consumed;
                }
                (Err(e), consumed) => {
                    results.push(Err(e));
                    total_consumed += consumed;
                }
            }
        }
    }

    /// Encode a collection of LtValue structures into a buffer.
    /// Even if the encoding fails, `buf` may still be modified by
    /// previous encoding successes.
    fn encode_all(
        iter: impl Iterator<Item = Self>,
        buf: &mut [u8],
    ) -> Result<(), crate::packet_encoding::Error> {
        let mut idx = 0;
        for item in iter {
            item.encode(&mut buf[idx..])?;
            idx += item.encoded_len();
        }
        Ok(())
    }
}

#[derive(Error, Debug, PartialEq)]
pub enum Error<Type: std::fmt::Debug + Into<u8> + Copy> {
    #[error("Buffer too short for next type")]
    MissingType,
    #[error("Buffer missing data indicated by length (type {0:?})")]
    MissingData(Type),
    #[error("Unrecognized type value for {0}: {1}")]
    UnrecognizedType(String, u8),
    #[error("Length of item ({0}) is outside allowed range for {1:?}: {2:?}")]
    LengthOutOfRange(u8, Type, std::ops::RangeInclusive<u8>),
    #[error("Error decoding type {0:?}: {1}")]
    TypeFailedToDecode(Type, crate::packet_encoding::Error),
}

impl<Type: std::fmt::Debug + Into<u8> + Copy> Error<Type> {
    pub fn type_value(&self) -> Option<u8> {
        match self {
            Self::MissingType => None,
            Self::MissingData(t)
            | Self::LengthOutOfRange(_, t, _)
            | Self::TypeFailedToDecode(t, _) => Some((*t).into()),
            Self::UnrecognizedType(_, value) => Some(*value),
        }
    }
}

impl<T: LtValue> Encodable for T {
    type Error = crate::packet_encoding::Error;

    fn encoded_len(&self) -> core::primitive::usize {
        2 + self.value_encoded_len() as usize
    }

    fn encode(&self, buf: &mut [u8]) -> core::result::Result<(), Self::Error> {
        if buf.len() < self.encoded_len() {
            return Err(crate::packet_encoding::Error::BufferTooSmall);
        }
        buf[0] = self.value_encoded_len() + 1;
        buf[1] = self.into_type().into();
        self.encode_value(&mut buf[2..self.encoded_len()])?;
        Ok(())
    }
}

impl<T> Decodable for T
where
    T: LtValue,
{
    type Error = Error<T::Type>;

    fn decode(buf: &[u8]) -> (core::result::Result<Self, Self::Error>, usize) {
        if buf.len() < 2 {
            return (Err(Error::MissingType), buf.len());
        }
        let indicated_len = buf[0] as usize;
        let too_short = buf.len() < indicated_len + 1;

        let Some(ty) = Self::type_from_octet(buf[1]) else {
            return (
                Err(Error::UnrecognizedType(Self::NAME.to_owned(), buf[1])),
                if too_short { buf.len() } else { indicated_len + 1 },
            );
        };
        if too_short {
            return (Err(Error::MissingData(ty)), buf.len());
        }
        let size_range = Self::length_range_from_type(ty);
        let remaining_len = (buf.len() - 1) as u8;
        if !size_range.contains(&remaining_len) {
            return (Err(Error::LengthOutOfRange(remaining_len, ty, size_range)), buf.len());
        }
        match Self::decode_value(&ty, &buf[2..=indicated_len]) {
            Err(e) => (Err(Error::TypeFailedToDecode(ty, e)), indicated_len + 1),
            Ok(s) => (Ok(s), indicated_len + 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Copy, Clone, PartialEq, Debug)]
    enum TestType {
        OneByte,
        TwoBytes,
        TwoBytesLittleEndian,
        UnicodeString,
        AlwaysError,
    }

    impl From<TestType> for u8 {
        fn from(value: TestType) -> Self {
            match value {
                TestType::OneByte => 1,
                TestType::TwoBytes => 2,
                TestType::TwoBytesLittleEndian => 3,
                TestType::UnicodeString => 4,
                TestType::AlwaysError => 0xFF,
            }
        }
    }

    #[derive(PartialEq, Debug)]
    enum TestValues {
        OneByte(u8),
        TwoBytes(u16),
        TwoBytesLittleEndian(u16),
        UnicodeString(String),
        AlwaysError,
    }

    impl LtValue for TestValues {
        type Type = TestType;

        const NAME: &'static str = "TestValues";

        fn type_from_octet(x: u8) -> Option<Self::Type> {
            match x {
                1 => Some(TestType::OneByte),
                2 => Some(TestType::TwoBytes),
                3 => Some(TestType::TwoBytesLittleEndian),
                4 => Some(TestType::UnicodeString),
                0xFF => Some(TestType::AlwaysError),
                _ => None,
            }
        }

        fn length_range_from_type(ty: Self::Type) -> std::ops::RangeInclusive<u8> {
            match ty {
                TestType::OneByte => 2..=2,
                TestType::TwoBytes => 3..=3,
                TestType::TwoBytesLittleEndian => 3..=3,
                TestType::UnicodeString => 2..=255,
                // AlwaysError fields can be any length (value will be thrown away)
                TestType::AlwaysError => 1..=255,
            }
        }

        fn into_type(&self) -> Self::Type {
            match self {
                TestValues::TwoBytes(_) => TestType::TwoBytes,
                TestValues::TwoBytesLittleEndian(_) => TestType::TwoBytesLittleEndian,
                TestValues::OneByte(_) => TestType::OneByte,
                TestValues::UnicodeString(_) => TestType::UnicodeString,
                TestValues::AlwaysError => TestType::AlwaysError,
            }
        }

        fn value_encoded_len(&self) -> u8 {
            match self {
                TestValues::TwoBytes(_) => 2,
                TestValues::TwoBytesLittleEndian(_) => 2,
                TestValues::OneByte(_) => 1,
                TestValues::UnicodeString(s) => s.len() as u8,
                TestValues::AlwaysError => 0,
            }
        }

        fn decode_value(
            ty: &Self::Type,
            buf: &[u8],
        ) -> Result<Self, crate::packet_encoding::Error> {
            match ty {
                TestType::OneByte => Ok(TestValues::OneByte(buf[0])),
                TestType::TwoBytes => {
                    Ok(TestValues::TwoBytes(u16::from_be_bytes([buf[0], buf[1]])))
                }
                TestType::TwoBytesLittleEndian => {
                    Ok(TestValues::TwoBytesLittleEndian(u16::from_le_bytes([buf[0], buf[1]])))
                }
                TestType::UnicodeString => {
                    Ok(TestValues::UnicodeString(String::from_utf8_lossy(buf).into_owned()))
                }
                TestType::AlwaysError => Err(crate::packet_encoding::Error::OutOfRange),
            }
        }

        fn encode_value(&self, buf: &mut [u8]) -> Result<(), crate::packet_encoding::Error> {
            if buf.len() < self.value_encoded_len() as usize {
                return Err(crate::packet_encoding::Error::BufferTooSmall);
            }
            match self {
                TestValues::TwoBytes(x) => {
                    [buf[0], buf[1]] = x.to_be_bytes();
                }
                TestValues::TwoBytesLittleEndian(x) => {
                    [buf[0], buf[1]] = x.to_le_bytes();
                }
                TestValues::OneByte(x) => {
                    buf[0] = *x;
                }
                TestValues::UnicodeString(s) => {
                    buf.copy_from_slice(s.as_bytes());
                }
                TestValues::AlwaysError => {
                    return Err(crate::packet_encoding::Error::InvalidParameter("test".to_owned()));
                }
            }
            Ok(())
        }
    }

    #[test]
    fn decode_twobytes() {
        let encoded = [0x03, 0x02, 0x10, 0x01, 0x03, 0x03, 0x10, 0x01];
        let (decoded, consumed) = TestValues::decode_all(&encoded);
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded[0], Ok(TestValues::TwoBytes(4097)));
        assert_eq!(decoded[1], Ok(TestValues::TwoBytesLittleEndian(272)));
    }

    #[test]
    fn decode_unrecognized() {
        let encoded = [0x03, 0x02, 0x10, 0x01, 0x03, 0x06, 0x10, 0x01];
        let (decoded, consumed) = TestValues::decode_all(&encoded);
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded[0], Ok(TestValues::TwoBytes(4097)));
        assert_eq!(decoded[1], Err(Error::UnrecognizedType("TestValues".to_owned(), 6)));
    }

    #[test]
    fn decode_wronglength() {
        let encoded = [0x03, 0x02, 0x10, 0x01, 0x04, 0x03, 0x10, 0x01];
        let (decoded, consumed) = TestValues::decode_all(&encoded);
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded[0], Ok(TestValues::TwoBytes(4097)));
        assert_eq!(decoded[1], Err(Error::MissingData(TestType::TwoBytesLittleEndian)));
    }

    #[test]
    fn encode_twobytes() {
        let value = TestValues::TwoBytes(0x0A0B);
        let mut buf = [0; 4];
        value.encode(&mut buf[..]).expect("should succeed");
        assert_eq!(buf, [0x03, 0x02, 0x0A, 0x0B]);
    }

    #[test]
    fn encode_all() {
        let value1 = TestValues::OneByte(0x0A);
        let value2 = TestValues::UnicodeString("Bluetooth".to_string());
        let mut buf = [0; 14];
        LtValue::encode_all(vec![value1, value2].into_iter(), &mut buf).expect("should succeed");
        assert_eq!(
            buf,
            [0x02, 0x01, 0x0a, 0x0a, 0x04, 0x42, 0x6c, 0x75, 0x65, 0x74, 0x6f, 0x6f, 0x74, 0x68]
        );
    }

    #[track_caller]
    fn u8char(c: char) -> u8 {
        c.try_into().unwrap()
    }

    #[test]
    fn decode_variable_lengths() {
        let encoded = [
            0x03,
            0x02,
            0x10,
            0x01,
            0x0A,
            0x04,
            u8char('B'),
            u8char('l'),
            u8char('u'),
            u8char('e'),
            u8char('t'),
            u8char('o'),
            u8char('o'),
            u8char('t'),
            u8char('h'),
            0x02,
            0x01,
            0x01,
        ];
        let (decoded, consumed) = TestValues::decode_all(&encoded);
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded[0], Ok(TestValues::TwoBytes(4097)));
        assert_eq!(decoded[1], Ok(TestValues::UnicodeString("Bluetooth".to_owned())));
        assert_eq!(decoded[2], Ok(TestValues::OneByte(1)));
    }

    #[test]
    fn decode_with_error() {
        let encoded = [0x03, 0x02, 0x10, 0x01, 0x02, 0xFF, 0xFF, 0x02, 0x01, 0x03];
        let (decoded, consumed) = TestValues::decode_all(&encoded);
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded[0], Ok(TestValues::TwoBytes(4097)));
        assert_eq!(
            decoded[1],
            Err(Error::TypeFailedToDecode(
                TestType::AlwaysError,
                crate::packet_encoding::Error::OutOfRange
            ))
        );
        assert_eq!(decoded[2], Ok(TestValues::OneByte(3)));
    }

    #[test]
    fn encode_with_error() {
        let value = TestValues::AlwaysError;
        let mut buf = [0; 10];
        assert!(matches!(
            value.encode(&mut buf),
            Err(crate::packet_encoding::Error::InvalidParameter(_)),
        ));

        let value1 = TestValues::TwoBytes(0x0A0B);
        let value2 = TestValues::OneByte(0x0A);
        let mut buf = [0; 2]; // not enough buffer space.
        LtValue::encode_all(vec![value1, value2].into_iter(), &mut buf).expect_err("should fail");
    }
}
