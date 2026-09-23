// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zerocopy::byteorder::big_endian::{U32 as BigEndianU32, U64 as BigEndianU64};
use zerocopy::{Immutable, IntoBytes};

const CHAIN_PARTITION_DESCRIPTOR_TAG: u64 = 4;

/// A VBMeta chain partition descriptor.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainPartitionDescriptor {
    /// The rollback index location.
    pub rollback_index_location: u32,
    /// The partition name.
    pub partition_name: String,
    /// The public key.
    pub public_key: Vec<u8>,
}

impl ChainPartitionDescriptor {
    /// Serialize the ChainPartitionDescriptor in the format expected by VBMeta.
    pub fn to_bytes(&self) -> Vec<u8> {
        // Header (92) + partition_name + public_key + padding.
        let name_len = self.partition_name.len();
        let key_len = self.public_key.len();

        let header = ChainPartitionDescriptorHeader::new(
            self.rollback_index_location,
            name_len as u32,
            key_len as u32,
        );

        let mut bytes = Vec::new();
        // The total size is header + name + key, rounded to multiple of 8.
        let encoding_len =
            (std::mem::size_of::<ChainPartitionDescriptorHeader>() + name_len + key_len)
                .next_multiple_of(8);
        bytes.reserve_exact(encoding_len);

        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(self.partition_name.as_bytes());
        bytes.extend_from_slice(&self.public_key);

        bytes.resize(encoding_len, 0);
        bytes
    }
}

#[derive(Clone, IntoBytes, Immutable, Debug, PartialEq)]
#[repr(C, packed)]
struct ChainPartitionDescriptorHeader {
    tag: BigEndianU64,
    num_bytes_following: BigEndianU64,
    rollback_index_location: BigEndianU32,
    partition_name_len: BigEndianU32,
    public_key_len: BigEndianU32,
    reserved: [u8; 64],
}

impl ChainPartitionDescriptorHeader {
    fn new(rollback_index_location: u32, partition_name_len: u32, public_key_len: u32) -> Self {
        // num_bytes_following is the size of the rest of the descriptor:
        // (76 + partition_name_len + public_key_len) rounded to multiple of 8.
        let num_bytes_following_unaligned = 76 + partition_name_len as u64 + public_key_len as u64;
        let num_bytes_following = num_bytes_following_unaligned.next_multiple_of(8);

        Self {
            tag: CHAIN_PARTITION_DESCRIPTOR_TAG.into(),
            num_bytes_following: num_bytes_following.into(),
            rollback_index_location: rollback_index_location.into(),
            partition_name_len: partition_name_len.into(),
            public_key_len: public_key_len.into(),
            reserved: [0u8; 64],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chain_partition_descriptor() {
        let desc = ChainPartitionDescriptor {
            rollback_index_location: 2,
            partition_name: "system".to_string(),
            public_key: vec![0x11, 0x22, 0x33, 0x44],
        };

        #[rustfmt::skip]
        let expected_bytes: [u8; 104] = [
            // Header
            // tag: 4
            0, 0, 0, 0, 0, 0, 0, 4,
            // num_bytes_following: 88 (0x58)
            0, 0, 0, 0, 0, 0, 0, 0x58,
            // rollback_index_location: 2
            0, 0, 0, 2,
            // partition_name_len: 6
            0, 0, 0, 6,
            // public_key_len: 4
            0, 0, 0, 4,
            // reserved: 64 bytes
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            // partition_name: "system"
            0x73, 0x79, 0x73, 0x74, 0x65, 0x6d,
            // public_key: [0x11, 0x22, 0x33, 0x44]
            0x11, 0x22, 0x33, 0x44,
            // padding
            0, 0,
        ];

        assert_eq!(desc.to_bytes(), expected_bytes.to_vec());
    }
}
