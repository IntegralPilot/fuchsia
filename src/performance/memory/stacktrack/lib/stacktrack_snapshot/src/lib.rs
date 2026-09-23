// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use flex_fuchsia_memory_stacktrack_client as fstacktrack_client;
use thiserror::Error;

mod snapshot;
pub use snapshot::{CallFrame, ExecutableRegion, Snapshot, StackTrace};

mod streamer;
pub use streamer::Streamer;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Missing expected field {} in {}", .field, .container)]
    MissingField { container: &'static str, field: &'static str },
    #[error("SnapshotReceiver stream ended unexpectedly")]
    UnexpectedEndOfStream,
    #[error("SnapshotReceiver stream contains an unknown element type")]
    UnexpectedElementType,
    #[error("SnapshotReceiver stream contains conflicting {} elements", .element_type)]
    ConflictingElement { element_type: &'static str },
    #[error("A page size was expected in the SnapshotReceiver stream")]
    PageSizeMissing,
    #[error("FIDL error: {}", .0)]
    FidlError(#[from] fidl::Error),
    #[cfg(feature = "fdomain")]
    #[error("FDomain error: {}", .0)]
    FDomainError(#[from] fdomain_client::Error),
    #[error("Collector error: {:?}", .0)]
    CollectorError(fstacktrack_client::CollectorError),
}

// Helper functions used by various tests in this crate.
#[cfg(test)]
pub(crate) mod test_helpers {
    #[cfg(feature = "fdomain")]
    pub fn create_client() -> std::sync::Arc<fdomain_client::Client> {
        fdomain_local::local_client_empty()
    }

    #[cfg(not(feature = "fdomain"))]
    pub fn create_client() -> fidl::endpoints::ZirconClient {
        fidl::endpoints::ZirconClient
    }
}
