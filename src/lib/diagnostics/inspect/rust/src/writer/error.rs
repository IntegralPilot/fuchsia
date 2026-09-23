// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use inspect_format::{BlockIndex, BlockType, Error as FormatError};

/// Errors that Inspect API functions can return.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("FIDL error: {0}")]
    Fidl(String),

    #[error("Failed to allocate vmo")]
    #[cfg(target_os = "fuchsia")]
    AllocateVmo(#[source] zx::Status),

    #[error("Failed to get vmo size")]
    #[cfg(target_os = "fuchsia")]
    GetVmoSize(#[source] zx::Status),

    #[error("VMO is not frozen")]
    #[cfg(target_os = "fuchsia")]
    VmoNotFrozen,

    #[error("Failed to duplicate vmo")]
    #[cfg(target_os = "fuchsia")]
    DuplicateVmoFailed(#[source] zx::Status),

    #[error("Failed to get frozen vmo; transaction_count={0}")]
    #[cfg(target_os = "fuchsia")]
    ConcurrentTransaction(usize),

    #[error("Failed to create child vmo")]
    #[cfg(target_os = "fuchsia")]
    CreateChildVmo(#[source] zx::Status),

    #[error("Expected a backing VMO for this operation, but found none")]
    VmoMissing,

    #[error("Failed to free {value_type} index={index}")]
    Free {
        value_type: &'static str,
        index: BlockIndex,
        #[source]
        error: Box<Error>,
    },

    #[error("Failed to create {value_type}")]
    Create {
        value_type: &'static str,
        #[source]
        error: Box<Error>,
    },

    #[error("{0} is no-op")]
    NoOp(&'static str),

    #[error("Failed to create the internal heap")]
    CreateHeap(#[source] Box<Error>),

    #[error("Failed to create the internal state")]
    CreateState(#[source] Box<Error>),

    #[error("Attempted to free a FREE block at index {0}")]
    BlockAlreadyFree(BlockIndex),

    #[error("Invalid index {0}: {1}")]
    InvalidIndex(BlockIndex, &'static str),

    #[error("Heap already at its maximum size")]
    HeapMaxSizeReached,

    #[error("Cannot allocate block of size {0}. Exceeds maximum.")]
    BlockSizeTooBig(usize),

    #[error("Invalid block type at index {0}: {1:?}")]
    InvalidBlockType(BlockIndex, u8),

    #[error("Invalid block type at index {0}: {1}")]
    InvalidBlockTypeNumber(BlockIndex, u8),

    #[error("Invalid block type. Expected: {0}, actual: {1}")]
    UnexpectedBlockType(BlockType, BlockType),

    #[error("Invalid block type. Expected: {0}, got: {1}")]
    UnexpectedBlockTypeRepr(&'static str, BlockType),

    #[error("Expected lock state locked={0}")]
    ExpectedLockState(bool),

    #[error("Invalid order {0}")]
    InvalidBlockOrder(usize),

    #[error("Invalid order {0} at index {1}")]
    InvalidBlockOrderAtIndex(u8, BlockIndex),

    #[error("Cannot swap blocks of different order or container")]
    InvalidBlockSwap,

    #[error("Expected a valid entry type for the array at index {0}")]
    InvalidArrayType(BlockIndex),

    #[error("Invalid array index: {0}")]
    InvalidArrayIndex(usize),

    #[error("{slots} exceeds the maximum number of slots for order {order}: {max_capacity}")]
    ArrayCapacityExceeded { slots: usize, order: u8, max_capacity: usize },

    #[error("Invalid {value_type} flags={flags} at index {index}")]
    InvalidFlags { value_type: &'static str, flags: u8, index: BlockIndex },

    #[error("Name is not utf8")]
    NameNotUtf8,

    #[error("Failed to convert array slots to usize")]
    FailedToConvertArraySlotsToUsize,

    #[error("Format error")]
    VmoFormat(#[source] FormatError),

    #[error("Cannot adopt into different VMO")]
    AdoptionIntoWrongVmo,

    #[error("Cannot adopt ancestor")]
    AdoptAncestor,
}

impl From<FormatError> for Error {
    fn from(error: FormatError) -> Self {
        Self::VmoFormat(error)
    }
}

impl Error {
    pub fn fidl(err: anyhow::Error) -> Self {
        Self::Fidl(format!("{err}"))
    }

    pub fn free(value_type: &'static str, index: BlockIndex, error: Error) -> Self {
        Self::Free { value_type, index, error: Box::new(error) }
    }

    pub fn create(value_type: &'static str, error: Error) -> Self {
        Self::Create { value_type, error: Box::new(error) }
    }

    pub fn invalid_index(index: BlockIndex, reason: &'static str) -> Self {
        Self::InvalidIndex(index, reason)
    }

    pub fn invalid_flags(value_type: &'static str, flags: u8, index: BlockIndex) -> Self {
        Self::InvalidFlags { value_type, flags, index }
    }

    pub fn array_capacity_exceeded(slots: usize, order: u8, max_capacity: usize) -> Self {
        Self::ArrayCapacityExceeded { slots, order, max_capacity }
    }
}
