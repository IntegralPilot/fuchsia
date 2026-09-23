// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bstr::BStr;
use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::fmt::Write;
use std::ops::Deref;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};
use zx_types::ZX_MAX_NAME_LEN;

// Performance note: rust string is made of a struct and a heap allocation which uses 3 and 2 64bit
// values for struct fields and heap block header for a total of 40 bytes. Hence it is always more
// efficient to use ZXName.

// TODO(b/411121120): remove this struct when zx::Name is usable on host.

/// Zircon resource name with a maximum length of `ZX_MAX_NAME_LEN - 1`.
#[derive(
    Default,
    Eq,
    Hash,
    FromBytes,
    IntoBytes,
    Immutable,
    KnownLayout,
    PartialEq,
    PartialOrd,
    Ord,
    Clone,
)]
pub struct ZXName([u8; ZX_MAX_NAME_LEN]);

#[derive(Debug, PartialEq)]
pub enum Error {
    InvalidArgument,
}

impl ZXName {
    pub fn as_bytes(&self) -> &[u8] {
        match self.0.iter().position(|&b| b == 0) {
            Some(index) => &self.0[..index],
            None => &self.0[..],
        }
    }

    pub fn as_bstr(&self) -> &BStr {
        BStr::new(self.as_bytes())
    }

    // Null-terminated string, followed by zero-padding.
    pub fn buffer(&self) -> &[u8; ZX_MAX_NAME_LEN] {
        &self.0
    }

    /// Creates a new instance from a byte slice.
    /// Returns an error if the input is too long or contains interior null bytes.
    pub const fn try_from_bytes(b: &[u8]) -> Result<Self, Error> {
        if b.len() >= ZX_MAX_NAME_LEN {
            return Err(Error::InvalidArgument);
        }

        let mut inner = [0u8; ZX_MAX_NAME_LEN];
        let mut i = 0;
        while i < b.len() {
            if b[i] == 0 {
                return Err(Error::InvalidArgument);
            }
            inner[i] = b[i];
            i += 1;
        }

        Ok(Self(inner))
    }

    pub fn from_string_lossy(s: &str) -> Self {
        Self::from_bytes_lossy(s.as_bytes())
    }

    #[inline]
    pub const fn from_bytes_lossy(b: &[u8]) -> Self {
        let to_copy = if b.len() <= ZX_MAX_NAME_LEN - 1 { b.len() } else { ZX_MAX_NAME_LEN - 1 };

        let mut inner = [0u8; ZX_MAX_NAME_LEN];
        let mut source_idx = 0;
        let mut dest_idx = 0;
        while source_idx < to_copy {
            if b[source_idx] != 0 {
                inner[dest_idx] = b[source_idx];
                dest_idx += 1;
            }
            source_idx += 1;
        }

        Self(inner)
    }
}

impl std::fmt::Display for ZXName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Escapes non utf-8 sequences.
        std::fmt::Display::fmt(self.as_bstr(), f)
    }
}

impl std::fmt::Debug for ZXName {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_bstr(), f)
    }
}

impl From<&ZXName> for ZXName {
    fn from(name_ref: &ZXName) -> Self {
        name_ref.clone()
    }
}

impl Deref for ZXName {
    type Target = [u8; ZX_MAX_NAME_LEN];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Serializes as an utf-8 string replacing invalid utf-8 sequences
/// with `\x` followed by the hex byte value. `\` are escaped as `\\`
impl Serialize for ZXName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut res = String::new();
        for chunk in self.as_bstr().utf8_chunks() {
            for ch in chunk.valid().chars() {
                match ch {
                    '\\' => {
                        write!(res, "\\\\").unwrap();
                    }
                    _ => res.push(ch),
                }
            }
            for byte in chunk.invalid() {
                write!(res, "\\x{:02X}", byte).unwrap();
            }
        }
        serializer.serialize_str(&res)
    }
}

/// Deserializes a byte sequence from the utf-8 representation.
impl<'de> Deserialize<'de> for ZXName {
    fn deserialize<D>(deserializer: D) -> Result<ZXName, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ZXNameVisitor;

        impl<'de> Visitor<'de> for ZXNameVisitor {
            type Value = ZXName;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an string")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let mut result = Vec::new();
                let mut chars = v.as_bytes().iter();
                loop {
                    match chars.next() {
                        None => break,
                        Some(b'\\') => match chars.next() {
                            None => return Err(E::custom("Character expected after '\\' escape")),
                            Some(b'x') => result.push(
                                u8::from_str_radix(
                                    str::from_utf8(&[
                                        *chars.next().ok_or_else(|| {
                                            E::custom("Hex characters expected after '\\x'")
                                        })?,
                                        *chars.next().ok_or_else(|| {
                                            E::custom("Hex characters expected after '\\x'")
                                        })?,
                                    ])
                                    .map_err(|_| E::custom("Invalid utf-8 sequence after '\\x'"))?,
                                    16,
                                )
                                .map_err(|_| E::custom("Invalid hex pair after '\\x'"))?,
                            ),
                            Some(v) => result.push(*v),
                        },
                        Some(u) => {
                            result.push(*u);
                        }
                    }
                }
                Ok(ZXName::from_bytes_lossy(&result))
            }
        }
        deserializer.deserialize_str(ZXNameVisitor {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::RngExt as _;
    #[test]
    fn empty_name() {
        for empty in [
            &ZXName::try_from_bytes(b"").unwrap(),
            &ZXName::from_bytes_lossy(b""),
            &ZXName::from_string_lossy(""),
            ZXName::ref_from_bytes(&[0u8; ZX_MAX_NAME_LEN]).unwrap(),
        ] {
            assert_eq!(0, empty.as_bstr().len());
            assert_eq!(empty, empty);
            assert_eq!("", empty.to_string());
            assert_eq!(&[0u8; ZX_MAX_NAME_LEN], empty.buffer());
            assert_eq!(&[0u8; ZX_MAX_NAME_LEN], &**empty);
        }
    }

    #[test]
    fn just_fit() {
        let data = "abcdefghijklmnopqrstuvwxyz01234";
        for name in [
            &ZXName::try_from_bytes(data.as_bytes()).unwrap(),
            &ZXName::from_bytes_lossy(data.as_bytes()),
            &ZXName::from_string_lossy(data),
            ZXName::ref_from_bytes(b"abcdefghijklmnopqrstuvwxyz01234\0").unwrap(),
        ] {
            assert_eq!("abcdefghijklmnopqrstuvwxyz01234", name.to_string());
            assert_eq!(ZX_MAX_NAME_LEN - 1, name.to_string().len());
            assert_eq!(b"abcdefghijklmnopqrstuvwxyz01234\0", name.buffer());
            assert_eq!(b"abcdefghijklmnopqrstuvwxyz01234\0", &**name);
        }
    }

    #[test]
    fn too_long() {
        let data = "abcdefghijklmnopqrstuvwxyz012345";
        assert_eq!(Result::Err(Error::InvalidArgument), ZXName::try_from_bytes(data.as_bytes()));

        for name in [ZXName::from_bytes_lossy(data.as_bytes()), ZXName::from_string_lossy(data)] {
            assert_eq!("abcdefghijklmnopqrstuvwxyz01234", name.to_string());
            assert_eq!(ZX_MAX_NAME_LEN - 1, name.to_string().len());
        }
    }

    #[test]
    fn zero_inside() {
        let data = b"abc\0def\0\0\0";
        assert_eq!(Err(Error::InvalidArgument), ZXName::try_from_bytes(data));
        assert_eq!("abcdef", ZXName::from_bytes_lossy(data).to_string());
    }

    #[test]
    fn not_utf8() {
        let data: [u8; 2] = [0xff, 0xff];
        assert_eq!("\u{FFFD}\u{FFFD}", ZXName::from_bytes_lossy(&data).to_string());
    }

    #[test]
    fn test_serialize() {
        assert_eq!("\"abc\"", serde_json::to_string(&ZXName::from_string_lossy("abc")).unwrap());
        assert_eq!(
            "\"\\n\\t\\r'\\\"\\\\\\\\\"",
            serde_json::to_string(&ZXName::from_string_lossy("\n\t\r'\"\\")).unwrap()
        );
        assert_eq!(
            r#""aĀ(\\xC3)""#,
            serde_json::to_string(&ZXName::from_bytes_lossy(&[b'a', 0xc4, 0x80, b'(', 0xc3, b')']))
                .unwrap()
        );
    }

    #[test]
    fn test_deserialize() {
        assert!(
            format!("{:?}", serde_json::from_str::<ZXName>(r#""\\""#))
                .contains("Character expected after '\\\\'")
        );
        assert!(
            format!("{:?}", serde_json::from_str::<ZXName>(r#""\\x""#))
                .contains("Hex characters expected after")
        );
        assert!(
            format!("{:?}", serde_json::from_str::<ZXName>(r#""\\x1""#))
                .contains("Hex characters expected after")
        );
        assert!(
            format!("{:?}", serde_json::from_str::<ZXName>(r#""\\x1x""#))
                .contains("Invalid hex pair after")
        );
    }

    #[test]
    fn test_fuzz_serialize() {
        let mut rng = rand::rng();
        for _ in 0..100000 {
            let byte_vec: Vec<u8> =
                (0..rng.random_range(0..32)).map(|_| rng.random::<u8>()).collect();
            let before = ZXName::from_bytes_lossy(&byte_vec);
            let json = serde_json::to_string(&before).unwrap();
            let after: ZXName = serde_json::from_str(&json).expect("deserialization works");
            assert_eq!(before, after);
        }
    }
}
