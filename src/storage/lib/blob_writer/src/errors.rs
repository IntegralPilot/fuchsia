// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;
use zx::Status;

/// The error type used by the `BlobWriter` for the create() command.
#[derive(Clone, Debug, Error)]
pub enum CreateError {
    /// A FIDL error occurred.
    #[error(transparent)]
    Fidl(#[from] fidl::Error),
    /// There was a problem getting the shared vmo for the blob.
    #[error("failed to get vmo")]
    GetVmo(#[from] Status),
    /// An error occurred while getting the size of the vmo.
    #[error("failed to get size of vmo")]
    GetSize(#[source] Status),
}

/// The error type used by the `BlobWriter` for the write() command.
#[derive(Clone, Debug, Error)]
pub enum WriteError {
    /// A FIDL error occurred.
    #[error(transparent)]
    Fidl(#[from] fidl::Error),
    /// The outstanding writes queue ended prematurely.
    #[error("outstanding writes queue ended prematurely")]
    QueueEnded,
    /// An error occurred while making a BytesReady request to the BlobWriter server.
    #[error("failed to complete a BytesReady request")]
    BytesReady(#[source] Status),
    /// An error occurred while writing to the vmo.
    #[error("failed to write to the vmo")]
    VmoWrite(#[source] Status),
    /// Client tried to write past the expected end of the blob.
    #[error("client tried to write past the expected end of the blob")]
    EndOfBlob,
}
