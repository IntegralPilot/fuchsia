// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::directory::entry::DirectoryEntry;
use crate::directory::entry_container::Directory;
use crate::name::Name;
use name::ParseNameError;
use std::sync::Arc;
use thiserror::Error;
use zx_status::Status;

/// An entry with the same name already exists in the directory.
#[derive(Error, Debug)]
#[error("An entry with the same name already exists in the directory")]
pub struct AlreadyExists;

impl Into<Status> for AlreadyExists {
    fn into(self) -> Status {
        Status::ALREADY_EXISTS
    }
}

/// The entry identified by `name` is not a directory.
#[derive(Error, Debug)]
#[error("The specified entry is not a directory")]
pub struct NotDirectory;

impl Into<Status> for NotDirectory {
    fn into(self) -> Status {
        Status::NOT_DIR
    }
}

/// `DirectlyMutable` is a superset of `MutableDirectory` which also allows server-side management
/// of directory entries (via `add_entry` and `remove_entry`).
pub trait DirectlyMutable: Directory + Send + Sync {
    /// Adds a child entry to this directory.
    ///
    /// Possible errors are:
    ///   * `ZX_ERR_INVALID_ARGS` or `ZX_ERR_BAD_PATH` if `name` is not a valid [`Name`].
    ///   * `ZX_ERR_ALREADY_EXISTS` if an entry with the same name is already present in the
    ///     directory.
    fn add_entry(
        &self,
        name: impl TryInto<Name, Error: Into<ParseNameError>>,
        entry: Arc<dyn DirectoryEntry>,
    ) -> Result<(), Status> {
        self.add_entry_may_overwrite(name, entry, false)
    }

    /// Adds a child entry to this directory. If `overwrite` is true, this function may overwrite
    /// an existing entry.
    ///
    /// Possible errors are:
    ///   * `ZX_ERR_INVALID_ARGS` or `ZX_ERR_BAD_PATH` if `name` is not a valid [`Name`].
    ///   * `ZX_ERR_ALREADY_EXISTS` if an entry with the same name is already present in the
    ///     directory and `overwrite` is false.
    fn add_entry_may_overwrite(
        &self,
        name: impl TryInto<Name, Error: Into<ParseNameError>>,
        entry: Arc<dyn DirectoryEntry>,
        overwrite: bool,
    ) -> Result<(), Status> {
        let name: Name = name.try_into().map_err(Into::into)?;
        self.add_entry_impl(name, entry, overwrite)
            .map_err(|_: AlreadyExists| Status::ALREADY_EXISTS)
    }

    /// Adds a child entry to this directory.
    fn add_entry_impl(
        &self,
        name: Name,
        entry: Arc<dyn DirectoryEntry>,
        overwrite: bool,
    ) -> Result<(), AlreadyExists>;

    /// Removes a child entry from this directory.  In case an entry with the matching name was
    /// found, the entry will be returned to the caller.  If `must_be_directory` is true, an error
    /// is returned if the entry is not a directory.
    ///
    /// Possible errors are:
    ///   * `ZX_ERR_INVALID_ARGS` or `ZX_ERR_BAD_PATH` if `name` is not a valid [`Name`].
    ///   * `ZX_ERR_NOT_DIR` if the entry identified by `name` is not a directory and
    ///     `must_be_directory` is true.
    fn remove_entry<NameT>(
        &self,
        name: NameT,
        must_be_directory: bool,
    ) -> Result<Option<Arc<dyn DirectoryEntry>>, Status>
    where
        NameT: Into<String>,
        Self: Sized,
    {
        let name: String = name.into();
        let name: Name = name.try_into()?;
        let entry = self
            .remove_entry_impl(name, must_be_directory)
            .map_err(|_: NotDirectory| Status::NOT_DIR)?;
        Ok(entry)
    }

    /// Removes a child entry from this directory.  In case an entry with the matching name was
    /// found, the entry will be returned to the caller.
    fn remove_entry_impl(
        &self,
        name: Name,
        must_be_directory: bool,
    ) -> Result<Option<Arc<dyn DirectoryEntry>>, NotDirectory>;
}
