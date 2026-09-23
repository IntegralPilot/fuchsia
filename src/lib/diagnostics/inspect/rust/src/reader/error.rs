// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::Error as WriterError;
use diagnostics_hierarchy::Error as HierarchyError;
use inspect_format::{BlockIndex, Error as FormatError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReaderError {
    #[error("FIDL error")]
    Fidl(#[source] anyhow::Error),

    #[error("Lazy node callback failed")]
    LazyCallback(#[source] anyhow::Error),

    #[error("expected header block on index 0")]
    MissingHeader,

    #[error("Cannot find parent block index {0}")]
    ParentIndexNotFound(BlockIndex),

    #[error("Malformed tree, no complete node with parent=0")]
    MalformedTree,

    #[error("VMO format error")]
    VmoFormat(#[from] FormatError),

    #[error("The VMO is in an invalid format")]
    InvalidVmo,

    #[error("Tried to read more slots than available at block index {0}")]
    AttemptedToReadTooManyArraySlots(BlockIndex),

    #[error("unexpected array entry type format: {0:?}")]
    UnexpectedArrayEntryFormat(u8),

    #[error("Failed to parse name at index {0}")]
    ParseName(BlockIndex),

    #[error("No blocks at BlockIndex {0}")]
    GetBlock(BlockIndex),

    #[error("Failed to get consistent snapshot")]
    InconsistentSnapshot,

    #[error("Header missing or is locked")]
    MissingHeaderOrLocked,

    #[error("Cannot read from no-op Inspector")]
    NoOpInspector,

    #[cfg(target_os = "fuchsia")]
    #[error("Failed to call vmo")]
    Vmo(#[from] zx::Status),

    #[error("Error creating node hierarchy")]
    Hierarchy(#[from] HierarchyError),

    #[error("Failed to duplicate vmo handle")]
    DuplicateVmo,

    #[error("Failed to fetch vmo from Tree content")]
    FetchVmo,

    #[error("Failed to load tree name {0}")]
    FailedToLoadTree(String),

    #[error("Timed out reading tree")]
    TreeTimedOut,

    #[error("Failed to lock inspector state")]
    FailedToLockState(#[source] WriterError),

    #[error("Offset out of bounds while reading")]
    OffsetOutOfBounds,

    #[error("Internal error in the reader library, used to avoid crashing Archivist")]
    Internal,
}
