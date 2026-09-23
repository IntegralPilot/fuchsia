// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use hex::{FromHex, FromHexError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use thiserror::Error;

mod iter;
pub use iter::*;

/// The size of a hash in bytes.
pub const HASH_SIZE: usize = 32;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FuchsiaMerkleMarker;
/// A digest created by the Fuchsia Merkle Tree hashing algorithm.
/// https://fuchsia.dev/fuchsia-src/concepts/packages/merkleroot
pub type Hash = GenericDigest<FuchsiaMerkleMarker>;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Sha256Marker;
/// A digest created by the Sha256 hashing algorithm.
pub type Sha256 = GenericDigest<Sha256Marker>;

/// The 32 byte digest of a hash function. The type parameter indicates the hash algorithm that was
/// used to compute the digest.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct GenericDigest<T> {
    digest: [u8; HASH_SIZE],
    type_: std::marker::PhantomData<T>,
}

impl<T> GenericDigest<T> {
    /// Obtain a slice of the bytes representing the hash.
    pub fn as_bytes(&self) -> &[u8] {
        &self.digest[..]
    }

    pub const fn from_array(arr: [u8; HASH_SIZE]) -> Self {
        Self { digest: arr, type_: std::marker::PhantomData::<T> }
    }
}

impl<T> std::str::FromStr for GenericDigest<T> {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self { digest: FromHex::from_hex(s)?, type_: std::marker::PhantomData::<T> })
    }
}

impl<'de, T> Deserialize<'de> for GenericDigest<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        std::str::FromStr::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl<T> From<[u8; HASH_SIZE]> for GenericDigest<T> {
    fn from(bytes: [u8; HASH_SIZE]) -> Self {
        GenericDigest { digest: bytes, type_: std::marker::PhantomData::<T> }
    }
}

impl<T> From<GenericDigest<T>> for [u8; HASH_SIZE] {
    fn from(hash: GenericDigest<T>) -> Self {
        hash.digest
    }
}

impl<T> TryFrom<&[u8]> for GenericDigest<T> {
    type Error = std::array::TryFromSliceError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self { digest: bytes.try_into()?, type_: std::marker::PhantomData::<T> })
    }
}

impl<T> TryFrom<&str> for GenericDigest<T> {
    type Error = ParseHashError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Ok(Self { digest: FromHex::from_hex(s)?, type_: std::marker::PhantomData::<T> })
    }
}

impl<T> TryFrom<String> for GenericDigest<T> {
    type Error = ParseHashError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl<T> From<GenericDigest<T>> for String {
    fn from(h: GenericDigest<T>) -> Self {
        hex::encode(h.digest)
    }
}

impl<T> fmt::Display for GenericDigest<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex::encode(self.digest).fmt(f)
    }
}

impl<T> Serialize for GenericDigest<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<T> fmt::Debug for GenericDigest<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Hash").field(&self.to_string()).finish()
    }
}

impl<T> std::ops::Deref for GenericDigest<T> {
    type Target = [u8; HASH_SIZE];

    fn deref(&self) -> &Self::Target {
        &self.digest
    }
}

/// An error encountered while parsing a [`Hash`].
#[derive(Copy, Clone, Debug, Error, PartialEq)]
pub struct ParseHashError(FromHexError);

impl fmt::Display for ParseHashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            FromHexError::InvalidStringLength => {
                write!(f, "{}, expected {} hex encoded bytes", self.0, HASH_SIZE)
            }
            _ => write!(f, "{}", self.0),
        }
    }
}

impl From<FromHexError> for ParseHashError {
    fn from(e: FromHexError) -> Self {
        ParseHashError(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::str::FromStr;

    proptest! {
        #[test]
        fn test_from_str_display(ref s in "[[:xdigit:]]{64}") {
            let hash = Hash::from_str(s).unwrap();
            let display = format!("{hash}");
            prop_assert_eq!(s.to_ascii_lowercase(), display);
        }

        #[test]
        fn test_rejects_odd_length_strings(ref s in "[[:xdigit:]][[:xdigit:]]{2}{0,128}") {
            prop_assert_eq!(Err(FromHexError::OddLength.into()), Hash::from_str(s));
        }

        #[test]
        fn test_rejects_incorrect_byte_count(ref s in "[[:xdigit:]]{2}{0,128}") {
            prop_assume!(s.len() != HASH_SIZE * 2);
            prop_assert_eq!(Err(FromHexError::InvalidStringLength.into()), Hash::from_str(s));
        }
    }
}
