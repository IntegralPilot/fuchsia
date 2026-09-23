// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::block_index::BlockIndex;
use crate::block_type::BlockType;

/// Errors that Inspect API functions can return.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("{slots} exceeds the maximum number of slots for order {order}: {max_capacity}")]
    ArrayCapacityExceeded { slots: usize, order: u8, max_capacity: usize },

    #[error("Array index out of bounds: {0}")]
    ArrayIndexOutOfBounds(usize),

    #[error("Expected lock state locked={0}")]
    ExpectedLockState(bool),

    #[error("Invalid order {0}")]
    InvalidBlockOrder(u8),

    #[error("Cannot swap blocks of different order or container")]
    InvalidBlockSwap,

    #[error("Invalid block type at index {0}: {1}")]
    InvalidBlockTypeNumber(BlockIndex, u8),

    #[error("Invalid {value_type} flags={flags} at index {index}")]
    InvalidFlags { value_type: &'static str, flags: u8, index: BlockIndex },

    #[error("Failed to convert array slots to usize")]
    FailedToConvertArraySlotsToUsize,

    #[error("Name is not utf8")]
    NameNotUtf8,

    #[error("Expected a valid entry type for the array at index {0}")]
    InvalidArrayType(BlockIndex),

    #[error("Invalid block type. Expected: {0}, actual: {1}")]
    UnexpectedBlockType(BlockType, BlockType),

    #[error("Invalid block type. Expected: {0}, got: {1}")]
    UnexpectedBlockTypeRepr(String, BlockType),

    #[error("Invalid reference count. Reference count must be in range (0, 2^32)")]
    InvalidReferenceCount,

    #[error("invalid buffer format: {0}")]
    InvalidBufferFormat(u8),

    #[error("Size (={0}) of the inspect VMO could not be written to the header")]
    SizeNotWritten(u32),

    #[error("Attempted to read or write a slice at an invalid offset: {0}")]
    InvalidOffset(usize),
}

impl Error {
    pub fn array_capacity_exceeded(slots: usize, order: u8, max_capacity: usize) -> Self {
        Self::ArrayCapacityExceeded { slots, order, max_capacity }
    }

    pub fn invalid_flags(value_type: &'static str, flags: u8, index: BlockIndex) -> Self {
        Self::InvalidFlags { value_type, flags, index }
    }
}
