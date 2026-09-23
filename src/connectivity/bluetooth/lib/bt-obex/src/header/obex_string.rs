// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::PacketError;

/// Represents the String type that is sent/received in an OBEX packet.
/// Encoded as null-terminated UTF-16 text.
/// Defined in OBEX 1.5. Section 2.1.
#[derive(Clone, Debug, PartialEq)]
pub struct ObexString(pub String);

impl ObexString {
    pub fn len(&self) -> usize {
        self.to_be_bytes().len()
    }

    pub fn to_be_bytes(&self) -> Vec<u8> {
        self.0
            .encode_utf16()
            .chain([0]) // Add the null terminator
            .map(|v| v.to_be_bytes())
            .flatten()
            .collect()
    }
}

impl From<String> for ObexString {
    fn from(src: String) -> ObexString {
        ObexString(src)
    }
}

impl From<&str> for ObexString {
    fn from(src: &str) -> ObexString {
        ObexString(src.to_string())
    }
}

impl TryFrom<&[u8]> for ObexString {
    type Error = PacketError;

    fn try_from(src: &[u8]) -> Result<Self, Self::Error> {
        if src.len() == 0 {
            return Ok(Self(String::new()));
        }

        let unicode_array = src
            .chunks_exact(2)
            .into_iter()
            .map(|a| u16::from_be_bytes([a[0], a[1]]))
            .collect::<Vec<u16>>();
        let mut text = String::from_utf16(&unicode_array).map_err(PacketError::external)?;
        // OBEX Strings are represented as null terminated Unicode text. See OBEX 1.5 Section 2.1.
        if !text.ends_with('\0') {
            return Err(PacketError::data("text missing null terminator"));
        }

        let _ = text.pop();
        Ok(Self(text))
    }
}

impl ToString for ObexString {
    fn to_string(&self) -> String {
        self.0.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;

    fn expect_strings_equal(s1: ObexString, s2: String) {
        assert_eq!(s1.0, s2);
    }

    #[fuchsia::test]
    fn obex_string_from_bytes() {
        // Empty buf
        let converted = ObexString::try_from(&[][..]).expect("can convert to String");
        expect_strings_equal(converted, "".to_string());

        // Just a null terminator.
        let buf = [0x00, 0x00];
        let converted = ObexString::try_from(&buf[..]).expect("can convert to String");
        expect_strings_equal(converted, "".to_string());

        // "hello"
        let buf = [0x00, 0x68, 0x00, 0x65, 0x00, 0x6c, 0x00, 0x6c, 0x00, 0x6f, 0x00, 0x00];
        let converted = ObexString::try_from(&buf[..]).expect("can convert to String");
        expect_strings_equal(converted, "hello".to_string());

        // "bob" with two null terminators - only last is removed.
        let buf = [0x00, 0x62, 0x00, 0x6f, 0x00, 0x62, 0x00, 0x00, 0x00, 0x00];
        let converted = ObexString::try_from(&buf[..]).expect("can convert to String");
        expect_strings_equal(converted, "bob\0".to_string());
    }

    #[fuchsia::test]
    fn obex_string_missing_terminator_is_error() {
        // `foo.txt` with no null terminator.
        let buf =
            [0x00, 0x66, 0x00, 0x6f, 0x00, 0x6f, 0x00, 0x2e, 0x00, 0x74, 0x00, 0x78, 0x00, 0x74];
        let converted = ObexString::try_from(&buf[..]);
        assert_matches!(converted, Err(PacketError::Data(_)));
    }

    #[fuchsia::test]
    fn obex_string_invalid_utf16_is_error() {
        // Invalid utf-16 bytes.
        let buf =
            [0xd8, 0x34, 0xdd, 0x1e, 0x00, 0x6d, 0x00, 0x75, 0xd8, 0x00, 0x00, 0x69, 0x00, 0x63];
        let converted = ObexString::try_from(&buf[..]);
        assert_matches!(converted, Err(PacketError::Other(_)));
    }
}
