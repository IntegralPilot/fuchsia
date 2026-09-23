// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;
use zx::Status;

/// The error type used by the shutdown operation of a serving filesystem.
#[derive(Debug, Error)]
pub enum ShutdownError {
    /// An error occurred connecting to the Admin service.
    #[error(transparent)]
    ConnectToAdminService(#[from] anyhow::Error),
    /// A FIDL error occurred.
    #[error(transparent)]
    Fidl(#[from] fidl::Error),
}

/// The error type used by the query operation of a serving filesystem.
#[derive(Clone, Debug, Error)]
pub enum QueryError {
    /// A FIDL error occurred.
    #[error(transparent)]
    Fidl(#[from] fidl::Error),
    /// A request for filesystem info using the Directory protocol failed.
    #[error("failed to query filesystem with Directory: {0}")]
    DirectoryQuery(#[source] Status),
    /// The filesystem info returned by the Directory protocol was empty.
    #[error("empty filesystem info result")]
    DirectoryEmptyResult,
}
