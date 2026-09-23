// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A node which forwards open requests to a remote fuchsia.io server.

#[cfg(test)]
mod tests;

use crate::ObjectRequestRef;
#[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
use crate::ToObjectRequest as _;
use crate::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use crate::execution_scope::ExecutionScope;
use crate::path::Path;
#[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
use flex_client::fidl::ServerEnd;
use flex_fuchsia_io as fio;
use std::sync::Arc;
use zx_status::Status;

pub trait RemoteLike {
    /// Called when a fuchsia.io/Directory.Open request should be forwarded to the remote node.
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        path: Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status>;

    /// Returns whether the remote should be opened lazily for the given path.  If true, the remote
    /// won't be opened until the channel in the request is readable.  This request will *not* be
    /// considered lazy if the request requires an event such as OnRepresentation, and this method
    /// will by bypassed.
    fn lazy(&self, _path: &Path) -> bool {
        false
    }

    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    /// DEPRECATED - Do not implement unless required for backwards compatibility. Called when
    /// forwarding fuchsia.io/Directory.DeprecatedOpen requests.
    fn deprecated_open(
        self: Arc<Self>,
        _scope: ExecutionScope,
        flags: fio::OpenFlags,
        _path: Path,
        server_end: ServerEnd<fio::NodeMarker>,
    ) {
        flags.to_object_request(server_end.into_channel()).shutdown(Status::NOT_SUPPORTED);
    }
}

/// Creates a new node that forwards open requests a remote directory server.
pub fn remote_dir(dir: fio::DirectoryProxy) -> Arc<impl DirectoryEntry + RemoteLike> {
    Arc::new(RemoteDir { dir })
}

/// [`RemoteDir`] implements [`RemoteLike`]` by forwarding open requests to a remote directory.
struct RemoteDir {
    dir: fio::DirectoryProxy,
}

impl GetRemoteDir for RemoteDir {
    fn get_remote_dir(&self) -> Result<fio::DirectoryProxy, Status> {
        Ok(Clone::clone(&self.dir))
    }
}

/// A trait that can be implemented to return a directory proxy that should be used as a remote
/// directory.
pub trait GetRemoteDir {
    fn get_remote_dir(&self) -> Result<fio::DirectoryProxy, Status>;
}

impl<T: GetRemoteDir + Send + Sync + 'static> GetEntryInfo for T {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
    }
}

impl<T: GetRemoteDir + Send + Sync + 'static> DirectoryEntry for T {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_remote(self)
    }
}

impl<T: GetRemoteDir> RemoteLike for T {
    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    fn deprecated_open(
        self: Arc<Self>,
        _scope: ExecutionScope,
        flags: fio::OpenFlags,
        path: Path,
        server_end: ServerEnd<fio::NodeMarker>,
    ) {
        flags.to_object_request(server_end).handle(|object_request| {
            let _ = self.get_remote_dir()?.deprecated_open(
                flags,
                fio::ModeType::empty(),
                path.as_ref(),
                object_request.take().into_server_end(),
            );
            Ok(())
        });
    }

    fn open(
        self: Arc<Self>,
        _scope: ExecutionScope,
        path: Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        // There is nowhere to propagate any errors since we take the `object_request`. This is okay
        // as the channel will be dropped and closed if the wire call fails.
        let _ = self.get_remote_dir()?.open(
            path.as_ref(),
            flags,
            &object_request.options(),
            object_request.take().into_channel(),
        );
        Ok(())
    }
}
