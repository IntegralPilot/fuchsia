// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zerocopy::byteorder::big_endian::{U32 as BigEndianU32, U64 as BigEndianU64};
use zerocopy::{Immutable, IntoBytes};

const KERNEL_CMDLINE_DESCRIPTOR_TAG: u64 = 3;

/// A VBMeta kernel command line descriptor.
#[derive(Clone, Debug, PartialEq)]
pub struct KernelCmdlineDescriptor {
    /// Flags (reserved, should be 0).
    pub flags: u32,
    /// The kernel command line string.
    pub kernel_cmdline: String,
}

impl KernelCmdlineDescriptor {
    /// Creates a new kernel command line descriptor.
    pub fn new(flags: u32, kernel_cmdline: String) -> Self {
        Self { flags, kernel_cmdline }
    }

    /// Serialize the KernelCmdlineDescriptor in the format expected by VBMeta.
    pub fn to_bytes(&self) -> Vec<u8> {
        // Header + cmdline + padding.
        let encoding_len = (size_of::<KernelCmdlineDescriptorHeader>() + self.kernel_cmdline.len())
            .next_multiple_of(8);

        let mut bytes = Vec::new();
        bytes.reserve_exact(encoding_len);

        let header = KernelCmdlineDescriptorHeader::new(self.flags, &self.kernel_cmdline);
        bytes.extend_from_slice(header.as_bytes());

        bytes.extend_from_slice(self.kernel_cmdline.as_bytes());

        bytes.resize(encoding_len, 0);
        bytes
    }
}

#[repr(C)]
#[derive(Debug, Immutable, IntoBytes)]
struct KernelCmdlineDescriptorHeader {
    tag: BigEndianU64,
    num_bytes_following: BigEndianU64,
    flags: BigEndianU32,
    kernel_cmdline_num_bytes: BigEndianU32,
}

impl KernelCmdlineDescriptorHeader {
    fn new(flags: u32, kernel_cmdline: &str) -> Self {
        let cmdline_len = kernel_cmdline.len() as u64;
        // sizeof(flags) + sizeof(kernel_cmdline_num_bytes) + cmdline_len
        let num_bytes_following_unaligned = 4 + 4 + cmdline_len;
        Self {
            tag: KERNEL_CMDLINE_DESCRIPTOR_TAG.into(),
            num_bytes_following: num_bytes_following_unaligned.next_multiple_of(8).into(),
            flags: flags.into(),
            kernel_cmdline_num_bytes: (cmdline_len as u32).into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kernel_cmdline_descriptor() {
        let cmdline = "console=ttyS0".to_string();
        let desc = KernelCmdlineDescriptor::new(0, cmdline);

        #[rustfmt::skip]
        let expected_bytes: [u8; 40] = [
            // Header
            // tag: 3
            0, 0, 0, 0, 0, 0, 0, 0x03,
            // num_bytes_following: 24
            0, 0, 0, 0, 0, 0, 0, 0x18,
            // flags: 0
            0, 0, 0, 0,
            // kernel_cmdline_num_bytes: 13
            0, 0, 0, 0x0D,

            // Key: "console=ttyS0"
            0x63, 0x6F, 0x6E, 0x73, 0x6F, 0x6C, 0x65, 0x3D,
            0x74, 0x74, 0x79, 0x53, 0x30,

            // Padding
            0, 0, 0,
        ];

        assert_eq!(desc.to_bytes(), &expected_bytes);
    }
}
