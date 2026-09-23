// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Various constants used for Inspect.

/// Bytes per page
pub const PAGE_SIZE_BYTES: usize = 4096;

/// Size of the a VMO block header.
pub const HEADER_SIZE_BYTES: usize = 8;

/// Magic number for the Header block. "INSP" in UTF-8 little-endian.
pub const HEADER_MAGIC_NUMBER: u32 = 0x50534e49;

/// Version number for the Header block.
pub const HEADER_VERSION_NUMBER: u32 = 2;

/// Maximum number order of a block.
pub const NUM_ORDERS: u8 = 8;

/// The shift for order 0.
pub const MIN_ORDER_SHIFT: usize = 4;

/// The size for order 0.
pub const MIN_ORDER_SIZE: usize = 1 << MIN_ORDER_SHIFT; // 16 bytes

/// The shift for order NUM_ORDERS-1 (the maximum order)
pub const MAX_ORDER_SHIFT: usize = MIN_ORDER_SHIFT + (NUM_ORDERS as usize) - 1;

/// The size for order NUM_ORDERS-1 (the maximum order)
pub const MAX_ORDER_SIZE: usize = 1 << MAX_ORDER_SHIFT;

/// Default number of bytes for the VMO: 256K
pub const DEFAULT_VMO_SIZE_BYTES: usize = 256 * 1024;

/// Minimum size for the VMO: 4K
pub const MINIMUM_VMO_SIZE_BYTES: usize = 4 * 1024;

/// Maximum size for a VMO: 100MB
pub const MAX_VMO_SIZE: usize = 128 * 1024 * 1024;

/// Length in bytes of metadata in the payload of an array block.
pub const ARRAY_PAYLOAD_METADATA_SIZE_BYTES: usize = 8;

/// The number of bytes in the payload of a STRING_REFERENCE allotted to
/// the total length.
pub const STRING_REFERENCE_TOTAL_LENGTH_BYTES: usize = 4;

/// This generation count indicates a VMO is frozen.
/// It is even to allow creating an inspector that can write to the VMO.
pub const VMO_FROZEN: u64 = u64::MAX - 1;

/// The order of the header block.
pub const HEADER_ORDER: u8 = 1;

/// The maximum reference count for a STRING_REFERENCE block (24 bits).
pub const MAX_REFERENCE_COUNT: u32 = 0xFFFFFF;
