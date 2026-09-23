// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Module holding different kinds of pseudo directories and their building blocks.

use crate::directory::entry_container::Directory;
use crate::execution_scope::ExecutionScope;
use crate::object_request::ToObjectRequest as _;
use crate::path::Path;
use flex_client::fidl::ServerEnd;
use flex_fuchsia_io as fio;
use std::sync::Arc;

#[cfg(test)]
#[macro_use]
pub(crate) mod test_utils;

pub mod common;

pub mod immutable;
pub mod mutable;

mod connection;
pub mod dirents_sink;
pub mod entry;
pub mod entry_container;
pub mod helper;
pub mod read_dirents;
pub mod simple;
pub mod traversal_position;
pub mod watchers;

/// A directory can be open either as a directory or a node.
#[derive(Clone)]
pub struct DirectoryOptions {
    pub(crate) rights: fio::Operations,
}

impl DirectoryOptions {
    pub(crate) fn to_io1(&self) -> fio::OpenFlags {
        // Note that rights in io1 correspond to several different rights in io2. The *_STAR_DIR
        // constants defined in the protocol indicate which rights these flags map to. Note that
        // this is more strict than the checks in FileOptions::to_io1, as OpenFlags map to several
        // different io2 directory rights.
        let mut flags = fio::OpenFlags::empty();
        if self.rights.contains(fio::R_STAR_DIR) {
            flags |= fio::OpenFlags::RIGHT_READABLE;
        }
        if self.rights.contains(fio::W_STAR_DIR) {
            flags |= fio::OpenFlags::RIGHT_WRITABLE;
        }
        if self.rights.contains(fio::X_STAR_DIR) {
            flags |= fio::OpenFlags::RIGHT_EXECUTABLE;
        }
        flags
    }
}

impl From<&DirectoryOptions> for fio::Flags {
    fn from(options: &DirectoryOptions) -> Self {
        // There is 1:1 mapping between `fio::Operations` and `fio::Flags`.
        fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::from_bits_truncate(options.rights.bits())
    }
}

impl Default for DirectoryOptions {
    fn default() -> Self {
        DirectoryOptions { rights: fio::R_STAR_DIR }
    }
}

/// Helper function to serve a new connection to `directory` with `flags`. Errors will be
/// communicated via epitaph on the returned proxy. A new [`crate::execution_scope::ExecutionScope`]
/// will be created for the request.
pub fn serve<D: Directory + ?Sized>(
    directory: Arc<D>,
    scope: ExecutionScope,
    flags: fio::Flags,
) -> fio::DirectoryProxy {
    crate::serve_directory(directory, Path::dot(), scope, flags)
}

/// Helper function to serve a new connection to `directory` as read-only (i.e. with
/// [`fio::PERM_READABLE`]). Errors will be communicated via epitaph on the returned proxy. A new
/// [`crate::execution_scope::ExecutionScope`] will be created for the request.
pub fn serve_read_only<D: Directory + ?Sized>(
    directory: Arc<D>,
    scope: ExecutionScope,
) -> fio::DirectoryProxy {
    crate::serve_directory(directory, Path::dot(), scope, fio::PERM_READABLE)
}

/// Helper function to serve a connection to `directory` on `server_end` with specified `flags` and
/// `scope`. Errors will be communicated via epitaph on `server_end`.
pub fn serve_on<D: Directory + ?Sized>(
    directory: Arc<D>,
    flags: fio::Flags,
    scope: ExecutionScope,
    server_end: ServerEnd<fio::DirectoryMarker>,
) {
    let request = flags.to_object_request(server_end);
    request.handle(|request| directory.open(scope, Path::dot(), flags, request));
}
