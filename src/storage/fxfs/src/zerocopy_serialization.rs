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
/// A trait for container types that can be efficiently serialized as a byte slice and
/// reconstructed from a `Vec` of their elements.
pub trait SerializeAsBytes {
    type Inner: FromBytes + Immutable + Copy;

    /// Returns a byte slice that represents the container.
    fn as_bytes(&self) -> &[u8];

    /// Constructs a container from a `Vec` of its elements.
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

/// Serializes a container type as a byte slice.
pub fn serialize<T, S>(value: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    T: SerializeAsBytes,
    S: Serializer,
{
    serializer.serialize_bytes(value.as_bytes())
}

/// Deserializes a container type from a byte slice.
pub fn deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: SerializeAsBytes,
    D: Deserializer<'de>,
{
    // We shouldn't be deserializing zero-sized types.
    debug_assert!(size_of::<T::Inner>() > 0);

    // Bincode reads the bytes into a `Vec<u8>`. If the type we're deserializing has the same
    // alignment as `u8` then it's possible to take the `Vec<u8>` instead of copying the data.
    if align_of::<T::Inner>() == align_of::<u8>() {
        // Calls `visit_byte_buf`.
        deserializer.deserialize_byte_buf(Visitor(PhantomData::<T>))
    } else {
        // Calls `visit_bytes`.
        deserializer.deserialize_bytes(Visitor(PhantomData::<T>))
    }
}

/// A visitor for deserializing a container type from a byte slice.
struct Visitor<T>(PhantomData<T>);
impl<'de, T> serde::de::Visitor<'de> for Visitor<T>
where
    T: SerializeAsBytes,
{
    type Value = T;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "a byte array with a length that is a multiple of {}",
            size_of::<T::Inner>()
        )
    }

    fn visit_bytes<E>(self, bytes: &[u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if !bytes.len().is_multiple_of(size_of::<T::Inner>()) {
            return Err(E::custom(
                "input bytes are not a multiple of the size of the desired type",
            ));
        }
        let elements = bytes.len() / size_of::<T::Inner>();
        let mut vec: Vec<T::Inner> = Vec::with_capacity(elements);
        let dst = vec.spare_capacity_mut();
        unsafe {
            // SAFETY:
            //   - Both `bytes` and `dst` are nonnull, aligned, and nonoverlapping.
            //   - `bytes` is valid for reading `bytes.len()` bytes.
            //   - `dst` is valid for writing `bytes.len()` bytes.
            //   - `T::Inner` implements `FromBytes` which means that any bit pattern is valid and
            //     it's safe to initialize the elements this way.
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                dst.as_mut_ptr().cast::<u8>(),
                bytes.len(),
            );
            // SAFETY: All of the elements were initialized.
            vec.set_len(elements);
        }
        Ok(T::from_vec(vec))
    }

    fn visit_byte_buf<E>(self, bytes: Vec<u8>) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        // This method should only be invoked from `deserialize` when the inner type has `u8`
        // alignment.
        debug_assert!(align_of::<T::Inner>() == align_of::<u8>());
        // We shouldn't be deserializing zero-sized types.
        debug_assert!(size_of::<T::Inner>() > 0);

        if !bytes.len().is_multiple_of(size_of::<T::Inner>()) {
            return Err(E::custom(
                "input bytes are not a multiple of the size of the desired type",
            ));
        }
        let elements = bytes.len() / size_of::<T::Inner>();

        // Both the size and capacity of `bytes` must be a multiple of the size of `T::Inner` to be
        // able to change the type of the `Vec`.
        let vec = if !bytes.capacity().is_multiple_of(size_of::<T::Inner>()) {
            // Calling `Vec::into_boxed_slice` will realloc the allocation to drop the excess
            // capacity. If we're lucky, the new and old capacity will be in the same allocator
            // bucket and no reallocation will happen.
            let ptr = Box::into_raw(bytes.into_boxed_slice());
            // SAFETY:
            //   - All of the requirements for `Vec::from_raw_parts` are upheld:
            //     - Fxfs only uses the global allocator.
            //     - `u8` and `T::Inner` have the same alignment.
            //     - The size of the allocation is `size_of::<T::Inner>() * elements` because of the
            //       `Vec::into_boxed_slice` call.
            //     - All of the elements are initialized.
            //   - The pointer cast is safe because `T::Inner` implements `FromBytes`.
            unsafe { Vec::from_raw_parts(ptr.cast::<T::Inner>(), elements, elements) }
        } else {
            let (ptr, _size, capacity) = bytes.into_raw_parts();
            let capacity = capacity / size_of::<T::Inner>();
            // SAFETY:
            //   - All of the requirements for `Vec::from_raw_parts` are upheld:
            //     - Fxfs only uses the global allocator.
            //     - `u8` and `T::Inner` have the same alignment.
            //     - `size_of::<T::Inner>() * capacity` is the same as `bytes.capacity()`.
            //     - All of the elements are initialized.
            //   - The pointer cast is safe because `T::Inner` implements `FromBytes`.
            unsafe { Vec::from_raw_parts(ptr.cast::<T::Inner>(), elements, capacity) }
        };
        Ok(T::from_vec(vec))
    }
}

pub fn fingerprint<T: TypeFingerprint>() -> String {
    format!("AsBytes<{}>", T::fingerprint())
}

#[cfg(test)]
mod tests {
    use crate::serialized_types::{LATEST_VERSION, Versioned};
    use serde::{Deserialize, Serialize};

    #[fuchsia::test]
    fn test_boxed_slice_of_u8_is_the_same() {
        #[derive(Serialize, Deserialize, Versioned)]
        struct Regular(Box<[u8]>);
        #[derive(Serialize, Deserialize, Versioned, PartialEq, Eq, Debug)]
        struct Optimized(#[serde(with = "crate::zerocopy_serialization")] Box<[u8]>);

        let regular = Regular(vec![0, 1, 2, 3, 254, 255].into_boxed_slice());
        let mut regular_serialized = Vec::new();
        regular.serialize_into(&mut regular_serialized).unwrap();

        let optimized = Optimized(regular.0);
        let mut optimized_serialized = Vec::new();
        optimized.serialize_into(&mut optimized_serialized).unwrap();

        assert_eq!(regular_serialized, optimized_serialized);

        assert_eq!(
            Optimized::deserialize_from(&mut optimized_serialized.as_slice(), LATEST_VERSION)
                .unwrap(),
            optimized
        );
    }

    #[fuchsia::test]
    fn test_vec_of_u8_is_the_same() {
        #[derive(Serialize, Deserialize, Versioned)]
        struct Regular(Vec<u8>);
        #[derive(Serialize, Deserialize, Versioned, PartialEq, Eq, Debug)]
        struct Optimized(#[serde(with = "crate::zerocopy_serialization")] Vec<u8>);

        let regular = Regular(vec![0, 1, 2, 3, 254, 255]);
        let mut regular_serialized = Vec::new();
        regular.serialize_into(&mut regular_serialized).unwrap();

        let optimized = Optimized(regular.0);
        let mut optimized_serialized = Vec::new();
        optimized.serialize_into(&mut optimized_serialized).unwrap();

        assert_eq!(regular_serialized, optimized_serialized);

        assert_eq!(
            Optimized::deserialize_from(&mut optimized_serialized.as_slice(), LATEST_VERSION)
                .unwrap(),
            optimized
        );
    }

    #[fuchsia::test]
    fn test_boxed_slice_of_array_of_u8() {
        #[derive(Serialize, Deserialize, Versioned)]
        struct Regular(Box<[[u8; 4]]>);
        #[derive(Serialize, Deserialize, Versioned, PartialEq, Eq, Debug)]
        struct Optimized(#[serde(with = "crate::zerocopy_serialization")] Box<[[u8; 4]]>);

        let regular = Regular(vec![[0, 1, 2, 3], [252, 253, 254, 255]].into_boxed_slice());
        let mut regular_serialized = Vec::new();
        regular.serialize_into(&mut regular_serialized).unwrap();

        let optimized = Optimized(regular.0);
        let mut optimized_serialized = Vec::new();
        optimized.serialize_into(&mut optimized_serialized).unwrap();

        // The serialized data is the same, but the number of elements is different.
        assert_eq!(&regular_serialized[1..], &optimized_serialized[1..]);
        assert_eq!(regular_serialized[0], 2);
        assert_eq!(optimized_serialized[0], 8);

        assert_eq!(
            Optimized::deserialize_from(&mut optimized_serialized.as_slice(), LATEST_VERSION)
                .unwrap(),
            optimized
        );
    }

    #[fuchsia::test]
    fn test_vec_of_array_of_u8() {
        #[derive(Serialize, Deserialize, Versioned)]
        struct Regular(Vec<[u8; 4]>);
        #[derive(Serialize, Deserialize, Versioned, PartialEq, Eq, Debug)]
        struct Optimized(#[serde(with = "crate::zerocopy_serialization")] Vec<[u8; 4]>);

        let regular = Regular(vec![[0, 1, 2, 3], [252, 253, 254, 255]]);
        let mut regular_serialized = Vec::new();
        regular.serialize_into(&mut regular_serialized).unwrap();

        let optimized = Optimized(regular.0);
        let mut optimized_serialized = Vec::new();
        optimized.serialize_into(&mut optimized_serialized).unwrap();

        // The serialized data is the same, but the number of elements is different.
        assert_eq!(&regular_serialized[1..], &optimized_serialized[1..]);
        assert_eq!(regular_serialized[0], 2);
        assert_eq!(optimized_serialized[0], 8);

        assert_eq!(
            Optimized::deserialize_from(&mut optimized_serialized.as_slice(), LATEST_VERSION)
                .unwrap(),
            optimized
        );
    }

    #[fuchsia::test]
    fn test_vec_of_u64_round_trip() {
        #[derive(Serialize, Deserialize, Versioned, PartialEq, Eq, Debug)]
        struct Optimized(#[serde(with = "crate::zerocopy_serialization")] Vec<u64>);

        let optimized =
            Optimized(vec![0, 1, u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX - 1, u64::MAX]);
        let mut optimized_serialized = Vec::new();
        optimized.serialize_into(&mut optimized_serialized).unwrap();

        // 1 byte varint encoded length + 6 * 8 bytes per u64.
        assert_eq!(optimized_serialized.len(), 1 + 6 * 8);

        assert_eq!(
            Optimized::deserialize_from(&mut optimized_serialized.as_slice(), LATEST_VERSION)
                .unwrap(),
            optimized
        );
    }

    #[fuchsia::test]
    fn test_empty_vec() {
        #[derive(Serialize, Deserialize, Versioned, PartialEq, Eq, Debug)]
        struct OptimizedVec(#[serde(with = "crate::zerocopy_serialization")] Vec<u8>);
        let optimized = OptimizedVec(Vec::new());
        let mut buf = Vec::new();
        optimized.serialize_into(&mut buf).unwrap();
        assert_eq!(
            OptimizedVec::deserialize_from(&mut buf.as_slice(), LATEST_VERSION).unwrap(),
            optimized
        );
    }

    #[fuchsia::test]
    fn test_visit_byte_buf_capacity_mismatch() {
        // Create a Vec<u8> with capacity that is not a multiple of 4.
        let mut bytes = Vec::with_capacity(11);
        bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(bytes.len(), 8);
        assert!(bytes.capacity() >= 11);
        assert!(!bytes.capacity().is_multiple_of(size_of::<u32>()));

        // [u8; 4] has size 4, alignment 1.
        let visitor = super::Visitor(std::marker::PhantomData::<Vec<[u8; 4]>>);
        let result: Vec<[u8; 4]> =
            serde::de::Visitor::visit_byte_buf::<serde::de::value::Error>(visitor, bytes)
                .expect("visit_byte_buf failed");
        assert_eq!(&result, &[[1, 2, 3, 4], [5, 6, 7, 8]]);
    }

    #[fuchsia::test]
    fn test_visit_byte_buf_invalid_length() {
        let bytes = vec![1, 2, 3, 4, 5]; // Length 5 is not a multiple of 4.
        let visitor = super::Visitor(std::marker::PhantomData::<Vec<[u8; 4]>>);
        let result: Result<_, serde::de::value::Error> =
            serde::de::Visitor::visit_byte_buf(visitor, bytes);
        assert!(result.is_err());
    }

    #[fuchsia::test]
    fn test_visit_bytes_invalid_length() {
        let bytes = vec![1, 2, 3, 4, 5]; // Length 5 is not a multiple of 2.
        let visitor = super::Visitor(std::marker::PhantomData::<Vec<u16>>);
        let result: Result<_, serde::de::value::Error> =
            serde::de::Visitor::visit_bytes(visitor, &bytes);
        assert!(result.is_err());
    }

    #[fuchsia::test]
    fn test_visit_bytes_with_bad_alignment() {
        const E1: u64 = 0x0123456789ABCDEF;
        const E2: u64 = u64::MAX;
        // Scudo guarantees 16 byte alignment for allocations. Extra bytes are added at the front to
        // force the u64s to be misaligned.
        let mut bytes: Vec<u8> = vec![0, 1, 2];
        bytes.extend_from_slice(&E1.to_le_bytes());
        bytes.extend_from_slice(&E2.to_le_bytes());
        assert!(!(&bytes[3..]).as_ptr().cast::<u64>().is_aligned());
        let visitor = super::Visitor(std::marker::PhantomData::<Vec<u64>>);
        let result: Vec<u64> =
            serde::de::Visitor::visit_bytes::<serde::de::value::Error>(visitor, &bytes[3..])
                .expect("visit_bytes failed");
        assert_eq!(&result, &[E1, E2]);
    }

    #[fuchsia::test]
    fn test_visit_bytes_with_empty_slice() {
        let visitor = super::Visitor(std::marker::PhantomData::<Vec<u64>>);
        let result: Vec<u64> =
            serde::de::Visitor::visit_bytes::<serde::de::value::Error>(visitor, &[])
                .expect("visit_bytes failed");
        assert!(result.is_empty());
    }
}
