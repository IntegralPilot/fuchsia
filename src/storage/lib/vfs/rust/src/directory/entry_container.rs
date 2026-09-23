// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! `EntryContainer` is a trait implemented by directories that allow manipulation of their
//! content.

use crate::directory::dirents_sink;
use crate::directory::traversal_position::TraversalPosition;
use crate::execution_scope::ExecutionScope;
use crate::node::Node;
use crate::object_request::ObjectRequestRef;
#[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
use crate::object_request::ToObjectRequest as _;
use crate::path::Path;
use flex_client::fidl::ServerEnd;
use flex_fuchsia_io as fio;
use futures::future::BoxFuture;
use std::any::Any;
use std::future::{Future, ready};
use std::sync::Arc;
use zx_status::Status;

mod private {
    use flex_fuchsia_io as fio;

    /// A type-preserving wrapper around a channel.
    #[derive(Debug)]
    pub struct DirectoryWatcher {
        #[cfg(not(feature = "fdomain"))]
        channel: zx::Channel,
        #[cfg(feature = "fdomain")]
        channel: flex_client::AsyncChannel,
    }

    impl DirectoryWatcher {
        /// Provides access to the underlying channel.
        #[cfg(not(feature = "fdomain"))]
        pub fn channel(&self) -> &zx::Channel {
            &self.channel
        }

        /// Provides access to the underlying channel.
        #[cfg(feature = "fdomain")]
        pub fn channel(&self) -> &flex_client::AsyncChannel {
            &self.channel
        }
    }

    impl From<flex_client::fidl::ServerEnd<fio::DirectoryWatcherMarker>> for DirectoryWatcher {
        fn from(server_end: flex_client::fidl::ServerEnd<fio::DirectoryWatcherMarker>) -> Self {
            Self { channel: server_end.into_channel() }
        }
    }
}

pub use private::DirectoryWatcher;

/// All directories implement this trait.  If a directory can be modified it should
/// also implement the `MutableDirectory` trait.
pub trait Directory: Node {
    /// Opens a connection to this item if the `path` is "." or a connection to an item inside
    /// this one otherwise.  `path` will not contain any "." or ".." components.
    ///
    /// `flags` corresponds to the fuchsia.io [`fio::Flags`] type. See fuchsia.io's Open method for
    /// more information regarding how flags are handled and what flag combinations are valid.
    ///
    /// If this method was initiated by a FIDL Open call, hierarchical rights are enforced at the
    /// connection layer.
    ///
    /// If the implementation takes `object_request`, it is then responsible for sending an
    /// `OnRepresentation` event when `flags` includes [`fio::Flags::FLAG_SEND_REPRESENTATION`].
    ///
    /// This method is called via either `Open` or `Reopen` fuchsia.io methods. Any errors returned
    /// during this process will be sent via an epitaph on the `object_request` channel before
    /// closing the channel.
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        path: Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status>;

    /// Same as [`Self::open`] but the implementation is async. This may be more efficient if the
    /// directory needs to do async work to open the connection.
    fn open_async(
        self: Arc<Self>,
        scope: ExecutionScope,
        path: Path,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> impl Future<Output = Result<(), Status>> + Send
    where
        Self: Sized,
    {
        ready(self.open(scope, path, flags, object_request))
    }

    /// Reads directory entries starting from `pos` by adding them to `sink`.
    /// Once finished, should return a sealed sink.
    fn read_dirents(
        &self,
        pos: &TraversalPosition,
        sink: Box<dyn dirents_sink::Sink>,
    ) -> impl Future<Output = Result<(TraversalPosition, Box<dyn dirents_sink::Sealed>), Status>> + Send
    where
        Self: Sized;

    /// Register a watcher for this directory.
    /// Implementations will probably want to use a `Watcher` to manage watchers.
    fn register_watcher(
        self: Arc<Self>,
        scope: ExecutionScope,
        mask: fio::WatchMask,
        watcher: DirectoryWatcher,
    ) -> Result<(), Status>;

    /// Unregister a watcher from this directory. The watcher should no longer
    /// receive events.
    fn unregister_watcher(self: Arc<Self>, key: usize);

    #[cfg(any(fuchsia_api_level_at_least = "PLATFORM", not(fuchsia_api_level_at_least = "32")))]
    /// DEPRECATED - Do not implement unless required for backwards compatibility. Called when
    /// handling a fuchsia.io/Directory.DeprecatedOpen request.
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

/// This trait indicates a directory that can be mutated by adding and removing entries.
/// This trait must be implemented to use a `MutableConnection`, however, a directory could also
/// implement the `DirectlyMutable` type, which provides a blanket implementation of this trait.
pub trait MutableDirectory: Directory + Send + Sync {
    /// Adds a child entry to this directory.  If the target exists, it should fail with
    /// ZX_ERR_ALREADY_EXISTS.
    fn link<'a>(
        self: Arc<Self>,
        _name: String,
        _source_dir: Arc<dyn Any + Send + Sync>,
        _source_name: &'a str,
    ) -> BoxFuture<'a, Result<(), Status>> {
        Box::pin(ready(Err(Status::NOT_SUPPORTED)))
    }

    /// Set the mutable attributes of this directory based on the values in `attributes`. If the
    /// directory does not support updating *all* of the specified attributes, implementations
    /// should fail with `ZX_ERR_NOT_SUPPORTED`.
    fn update_attributes(
        &self,
        attributes: fio::MutableNodeAttributes,
    ) -> impl Future<Output = Result<(), Status>> + Send
    where
        Self: Sized;

    /// Removes an entry from this directory.
    fn unlink(
        self: Arc<Self>,
        name: &str,
        must_be_directory: bool,
    ) -> impl Future<Output = Result<(), Status>> + Send
    where
        Self: Sized;

    /// Syncs the directory.
    fn sync(&self) -> impl Future<Output = Result<(), Status>> + Send
    where
        Self: Sized;

    /// Renames into this directory.
    fn rename(
        self: Arc<Self>,
        _src_dir: Arc<dyn MutableDirectory>,
        _src_name: Path,
        _dst_name: Path,
    ) -> BoxFuture<'static, Result<(), Status>> {
        Box::pin(ready(Err(Status::NOT_SUPPORTED)))
    }

    /// Creates a symbolic link.
    fn create_symlink(
        &self,
        _name: String,
        _target: Vec<u8>,
        _connection: Option<ServerEnd<fio::SymlinkMarker>>,
    ) -> impl Future<Output = Result<(), Status>> + Send
    where
        Self: Sized,
    {
        ready(Err(Status::NOT_SUPPORTED))
    }
}
