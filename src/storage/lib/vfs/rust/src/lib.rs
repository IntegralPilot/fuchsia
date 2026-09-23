// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia Rust Virtual File System (VFS) framework.
//!
//! This crate provides safe, asynchronous implementations of the `fuchsia.io` VFS protocols
//! (directories, files, services, symbolic links, and remote directory mounts).

#![recursion_limit = "1024"]

#[macro_use]
pub mod common;

pub mod directory;
pub mod execution_scope;
pub mod file;
pub mod node;
pub mod object_request;
pub mod path;
mod protocols;
pub mod remote;
mod request_handler;
pub mod service;
pub mod symlink;
pub mod temp_clone;
pub mod test_utils;
pub mod token_registry;
pub mod tree_builder;

pub use crate::common::CreationMode;
pub use crate::execution_scope::{ExecutionScope, WeakExecutionScope};
pub use crate::object_request::{ObjectRequest, ObjectRequestRef, ToObjectRequest};
pub use crate::path::Path;
pub use crate::protocols::ProtocolsExt;
pub use ::name;

#[cfg(test)]
use flex_test_placeholders as _;
#[cfg(all(test, feature = "fdomain"))]
use fuchsia_fs_fdomain as _;

use directory::entry_container::Directory;
use flex_fuchsia_io as fio;
use std::sync::Arc;

/// Helper function to serve a new connection to the directory at `path` under `root` with `flags`.
/// Errors will be communicated via epitaph on the returned proxy. A new [`ExecutionScope`] will be
/// created for the request.
///
/// To serve `root` itself, use [`crate::directory::serve`] or set `path` to [`Path::dot`].
pub fn serve_directory<D: Directory + ?Sized>(
    root: Arc<D>,
    path: Path,
    scope: ExecutionScope,
    flags: fio::Flags,
) -> fio::DirectoryProxy {
    let (proxy, server) = scope.domain().create_proxy::<fio::DirectoryMarker>();
    let request = flags.to_object_request(server);
    request.handle(|request| root.open(scope, path, flags, request));
    proxy
}

/// Helper function to serve a new connection to the file at `path` under `root` with `flags`.
/// Errors will be communicated via epitaph on the returned proxy. A new [`ExecutionScope`] will be
/// created for the request.
///
/// To serve an object that implements [`crate::file::File`], use [`crate::file::serve`].
pub fn serve_file<D: Directory + ?Sized>(
    root: Arc<D>,
    path: Path,
    scope: ExecutionScope,
    flags: fio::Flags,
) -> fio::FileProxy {
    let (proxy, server) = scope.domain().create_proxy::<fio::FileMarker>();
    let request = flags.to_object_request(server);
    request.handle(|request| root.open(scope, path, flags, request));
    proxy
}
