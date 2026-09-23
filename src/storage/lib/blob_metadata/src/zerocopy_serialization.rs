// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Efficient serialization and deserialization for `Vec<T>` and `Box<[T]>` where `T` supports
//! zerocopy.

use fprint::TypeFingerprint;
use serde::{Deserializer, Serializer};
use std::marker::PhantomData;
use zerocopy::{FromBytes, Immutable, IntoBytes};

// Only little endian is supported.
static_assertions::assert_cfg!(target_endian = "little");

pub trait SerializeAsBytes {
    type Inner: FromBytes + Immutable + Copy;

    fn as_bytes(&self) -> &[u8];
    fn from_vec(slice: Vec<Self::Inner>) -> Self;
}

impl<T: FromBytes + IntoBytes + Immutable + Copy> SerializeAsBytes for Vec<T> {
    type Inner = T;

    fn as_bytes(&self) -> &[u8] {
        self.as_slice().as_bytes()
    }

    fn from_vec(slice: Vec<T>) -> Self {
        slice
    }
}

impl<T: FromBytes + IntoBytes + Immutable + Copy> SerializeAsBytes for Box<[T]> {
    type Inner = T;

    fn as_bytes(&self) -> &[u8] {
        (&**self).as_bytes()
    }

    fn from_vec(slice: Vec<T>) -> Self {
        slice.into_boxed_slice()
    }
}

pub fn serialize<T, S>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    T: SerializeAsBytes,
    S: Serializer,
{
    serializer.serialize_bytes(value.as_bytes())
}

pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: SerializeAsBytes,
    D: Deserializer<'de>,
{
    struct Visitor<T: SerializeAsBytes>(PhantomData<T>);

    impl<'de, T: SerializeAsBytes> serde::de::Visitor<'de> for Visitor<T> {
        type Value = T;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a byte slice")
        }

        fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            if v.len() % std::mem::size_of::<T::Inner>() != 0 {
                return Err(E::custom("Invalid byte slice length for zerocopy type"));
            }
            let slice = unsafe {
                std::slice::from_raw_parts(
                    v.as_ptr().cast::<T::Inner>(),
                    v.len() / std::mem::size_of::<T::Inner>(),
                )
            };
            Ok(T::from_vec(slice.to_vec()))
        }
    }

    deserializer.deserialize_bytes(Visitor(PhantomData))
}

pub fn fingerprint<T: SerializeAsBytes + TypeFingerprint>() -> String {
    format!("AsBytes<{}>", T::fingerprint())
}
