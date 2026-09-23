// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zerocopy::byteorder::big_endian::U64 as BigEndianU64;
use zerocopy::{Immutable, IntoBytes};

const PROPERTY_DESCRIPTOR_TAG: u64 = 0;

/// A VBMeta property descriptor.
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyDescriptor {
    /// The property key.
    pub key: String,
    /// The property value.
    pub value: String,
}

impl PropertyDescriptor {
    /// Creates a new property descriptor given a (key, value) pair.
    pub fn new(key: String, value: String) -> Self {
        Self { key, value }
    }

    /// Serialize the PropertyDescriptor in the format expected by VBMeta.
    pub fn to_bytes(&self) -> Vec<u8> {
        // Header + (key + NUL) + (value + NUL) + padding.
        let encoding_len =
            (size_of::<PropertyDescriptorHeader>() + self.key.len() + 1 + self.value.len() + 1)
                .next_multiple_of(8);

        let mut bytes = Vec::new();
        bytes.reserve_exact(encoding_len);

        let header = PropertyDescriptorHeader::new(&self.key, &self.value);
        bytes.extend_from_slice(header.as_bytes());

        bytes.extend_from_slice(self.key.as_bytes());
        bytes.push(0);

        bytes.extend_from_slice(self.value.as_bytes());
        bytes.push(0);

        bytes.resize(encoding_len, 0);
        bytes
    }
}

#[repr(C)]
#[derive(Debug, Immutable, IntoBytes)]
struct PropertyDescriptorHeader {
    tag: BigEndianU64,
    num_bytes_following: BigEndianU64,
    key_num_bytes: BigEndianU64,
    value_num_bytes: BigEndianU64,
}

impl PropertyDescriptorHeader {
    fn new(key: &str, value: &str) -> Self {
        let key_len = key.len() as u64;
        let value_len = value.len() as u64;
        // sizeof(key_num_bytes) + sizeof(value_num_bytes) + (key_len + NUL) + (value_len + NUL)
        let num_bytes_following_unaligned = 8 + 8 + (key_len + 1) + (value_len + 1);
        Self {
            tag: PROPERTY_DESCRIPTOR_TAG.into(),
            num_bytes_following: num_bytes_following_unaligned.next_multiple_of(8).into(),
            key_num_bytes: key_len.into(),
            value_num_bytes: value_len.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_property_descriptor() {
        let key = "com.fuchsia.vbmeta.some_property".to_string();
        let value = "some_value".to_string();
        let prop = PropertyDescriptor::new(key, value);

        #[rustfmt::skip]
        let expected_bytes: [u8; 80] = [
            // Header
            // tag: 0
            0, 0, 0, 0, 0, 0, 0, 0,
            // num_bytes_following: 64
            0, 0, 0, 0, 0, 0, 0, 0x40,
            // key_num_bytes: 32
            0, 0, 0, 0, 0, 0, 0, 0x20,
            // value_num_bytes: 10
            0, 0, 0, 0, 0, 0, 0, 0x0a,

            // Key: "com.fuchsia.vbmeta.some_property"
            0x63, 0x6F, 0x6D, 0x2E, 0x66, 0x75, 0x63, 0x68,
            0x73, 0x69, 0x61, 0x2E, 0x76, 0x62, 0x6D, 0x65,
            0x74, 0x61, 0x2E, 0x73, 0x6F, 0x6D, 0x65, 0x5F,
            0x70, 0x72, 0x6F, 0x70, 0x65, 0x72, 0x74, 0x79,
            // NUL
            0,

            // Value: "some_value"
            0x73, 0x6F, 0x6D, 0x65, 0x5F, 0x76, 0x61, 0x6C,
            0x75, 0x65,
            // NUL
            0,

            // Padding
            0, 0, 0, 0,
        ];

        assert_eq!(prop.to_bytes(), &expected_bytes);
    }
}
