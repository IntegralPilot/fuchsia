// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! # Inspect bitfields
//!
//! This module contains the bitfield definitions of the [`Inspect VMO format`][inspect-vmo].
//!
//! [inspect-vmo]: https://fuchsia.dev/fuchsia-src/reference/diagnostics/inspect/vmo-format

use crate::block::{Block, BlockKind};
use crate::container::{ReadBytes, WriteBytes};
use std::ops::{Deref, DerefMut};

macro_rules! bitfield_fields {
    ($offset_fn:ident,) => {};

    ($offset_fn:ident,
     $(#[$attr:meta])* $type:ty,
     $name:ident: $msb:expr, $lsb:expr;
     $($rest:tt)*
    ) => {
        paste::item! {
            $(#[$attr])*
            #[inline]
            #[allow(clippy::identity_op)]
            pub fn $name<T: Deref<Target=Q>, Q: ReadBytes, K: BlockKind>(b: &Block<T, K>) -> $type {
                if let Some(value) = b.container.get_value::<u64>(b.$offset_fn()) {
                    static MASK : u64 = (1 << ($msb - $lsb + 1)) - 1;
                    // This cast is fine. We only deal with u8, u16, u32, u64 here.
                    return ((value >> ($lsb % 64)) & MASK ) as $type;
                }
                0
            }

            $(#[$attr])*
            #[inline]
            #[allow(clippy::identity_op)]
            pub fn [<set_ $name>]<T: Deref<Target=Q> + DerefMut<Target=Q>, Q: WriteBytes + ReadBytes, K: BlockKind>(
                   b: &mut Block<T, K>, value: $type) {
                let offset = b.$offset_fn();
                if let Some(num) = b.container.get_value::<u64>(offset) {
                    static MASK : u64 = (1u64 << ($msb - $lsb + 1)) - 1;
                    let new_val = (num & !MASK.checked_shl($lsb).unwrap_or(0)) |
                               (value as u64).checked_shl($lsb).unwrap_or(0);
                    let _ = b.container.set_value::<u64>(offset, new_val);
                }
            }
        }
        bitfield_fields!{$offset_fn, $($rest)*}
    };
}

macro_rules! block_bitfield {
    ($(#[$attr:meta])* struct $name:ident, $offset_fn:ident; $($rest:tt)*) => {
        $(#[$attr])*
        pub struct $name;

        impl $name {
            bitfield_fields!{$offset_fn, $($rest)*}

            /// Get the raw 64 bits of the header section of the block.
            pub fn value<T: Deref<Target=Q>, Q: ReadBytes, K: BlockKind>(b: &Block<T, K>) -> u64 {
                b.container.get_value::<u64>(b.$offset_fn()).unwrap_or(0)
            }

            /// Set the raw 64 bits of the header section of the block.
            #[inline]
            pub fn set_value<T: Deref<Target=Q> + DerefMut<Target=Q>, Q: WriteBytes + ReadBytes, K: BlockKind>(
                b: &mut Block<T, K>, value: u64
            ) {
                let offset = b.$offset_fn();
                let _ = b.container.set_value::<u64>(offset, value);
            }
        }
    };
}

block_bitfield! {
    /// Bitfields for writing and reading segments of the header and payload of
    /// inspect VMO blocks.
    /// Represents the header structure of an inspect VMO Block. Not to confuse with
    /// the `HEADER` block.
    struct HeaderFields, header_offset;

    /// The size of a block given as a bit shift from the minimum size.
    /// `size_in_bytes = 16 << order`. Separates blocks into classes by their (power of two) size.
    u8, order: 3, 0;

    /// The type of the block. Determines how the rest of the bytes are interpreted.
    /// - 0: Free
    /// - 1: Reserved
    /// - 2: Header
    /// - 3: Node
    /// - 4: Int value
    /// - 5: Uint value
    /// - 6: Double value
    /// - 7: Buffer value
    /// - 8: Extent
    /// - 9: Name
    /// - 10: Tombstone
    /// - 11: Array value
    /// - 12: Link value
    /// - 13: Bool value
    /// - 14: String Reference
    u8, block_type: 15, 8;

    /// Only for a `HEADER` block. The version number. Currently 1.
    u32, header_version: 31, 16;

    /// Only for a `HEADER` block. The magic number "INSP".
    u32, header_magic: 63, 32;

    /// Only for `*_VALUE` blocks. The index of the `NAME` block of associated with this value.
    u32, value_name_index: 63, 40;

    /// Only for `*_VALUE` blocks. The index of the parent of this value.
    u32, value_parent_index: 39, 16;

    // Only for RESERVED blocks
    u64, reserved_empty: 63, 16;

    // Only for TOMBSTONE blocks
    u64, tombstone_empty: 63, 16;

    /// Only for `FREE` blocks. The index of the next free block.
    u8, free_reserved: 7, 4;
    u32, free_next_index: 39, 16;
    u32, free_empty: 63, 40;

    /// Only for `NAME` blocks. The length of the string.
    u16, name_length: 27, 16;

    /// Only for `EXTENT` or `STRING_REFERENCE` blocks.
    /// The index of the next `EXTENT` block.
    u32, extent_next_index: 39, 16;

    /// Only for `STRING_REFERENCE` blocks.
    /// The number of active references to the string, including itself.
    u32, string_reference_count: 63, 40;
}

block_bitfield! {
    /// Represents the payload of inspect VMO Blocks (except for `EXTENT` and `NAME`).
    struct PayloadFields, payload_offset;

    /// Only for `BUFFER` or `STRING_REFERENCE` blocks. The total size of the buffer.
    u32, property_total_length:  31, 0;

    /// Only for `BUFFER` blocks. The index of the first `EXTENT` block of this buffer.
    u32, property_extent_index: 59, 32;

    /// Only for `BUFFER` blocks. The buffer flags of this block indicating its display format.
    /// 0: utf-8 string
    /// 1: binary array
    u8, property_flags: 63, 60;

    /// Only for `ARRAY_VALUE` blocks. The type of each entry in the array (int, uint, double).
    /// 0: Int
    /// 1: Uint
    /// 2: Double
    u8, array_entry_type: 3, 0;

    /// Only for `ARRAY_VALUE` blocks. The display format of the block (default, linear histogram,
    /// exponential histogram)
    /// 0: Regular array
    /// 1: Linear histogram
    /// 2: Exponential histogram
    u8, array_flags: 7, 4;

    /// Only for `ARRAY_VALUE` blocks. The nmber of entries in the array.
    u8, array_slots_count: 15, 8;

    /// Only for `LINK_VALUE` blocks. Index of the content of this link (as a `NAME` node)
    u32, content_index: 19, 0;

    /// Only for `LINK_VALUE`. Instructs readers whether to use child or inline disposition.
    /// 0: child
    /// 1: inline
    u8, disposition_flags: 63, 60;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockIndex, Buffer, Header};

    #[fuchsia::test]
    fn test_header() {
        let mut container = [0u8; 16];
        let mut block = Block::<_, Header>::new(&mut container, BlockIndex::EMPTY);
        let magic = 0x494e5350;
        HeaderFields::set_order(&mut block, 13);
        HeaderFields::set_block_type(&mut block, 3);
        HeaderFields::set_header_version(&mut block, 1);
        HeaderFields::set_header_magic(&mut block, magic);
        assert_eq!(HeaderFields::order(&block), 13);
        assert_eq!(HeaderFields::header_version(&block), 1);
        assert_eq!(HeaderFields::header_magic(&block), magic);
        assert_eq!(HeaderFields::value(&block), 0x494e53500001030d);
    }

    #[fuchsia::test]
    fn test_payload() {
        let mut container = [0u8; 16];
        let mut block = Block::<_, Buffer>::new(&mut container, BlockIndex::EMPTY);
        PayloadFields::set_property_total_length(&mut block, 0xab);
        PayloadFields::set_property_extent_index(&mut block, 0x1234);
        PayloadFields::set_property_flags(&mut block, 3);
        assert_eq!(PayloadFields::property_total_length(&block), 0xab);
        assert_eq!(PayloadFields::property_extent_index(&block), 0x1234);
        assert_eq!(PayloadFields::property_flags(&block), 3);
        assert_eq!(PayloadFields::value(&block), 0x30001234000000ab);
    }
}
