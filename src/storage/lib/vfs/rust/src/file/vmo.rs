// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of a file backed by a VMO buffer shared by all the file connections.

#[cfg(test)]
mod tests;

use crate::ObjectRequestRef;
use crate::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use crate::execution_scope::ExecutionScope;
use crate::file::common::vmo_flags_to_rights;
use crate::file::{File, FileLike, FileOptions, GetVmo, StreamIoConnection, SyncMode};
use crate::node::Node;
use flex_fuchsia_io as fio;
use std::sync::Arc;
use zx::{Status, Vmo};

/// Creates a new read-only `VmoFile` with the specified `content`.
///
/// ## Panics
///
/// This function panics if a VMO could not be created, or if `content` could not be written to the
/// VMO.
///
/// ## Examples
/// ```
/// // Using static data:
/// let from_str = read_only("str");
/// let from_bytes = read_only(b"bytes");
/// // Using owned data:
/// let from_string = read_only(String::from("owned"));
/// let from_vec = read_only(vec![0u8; 2]);
/// ```
pub fn read_only(content: impl AsRef<[u8]>) -> Arc<VmoFile> {
    VmoFile::from_bytes(content.as_ref())
}

/// Implementation of a VMO-backed file in a virtual file system.
pub struct VmoFile {
    /// Specifies if the file can be opened as executable.
    executable: bool,

    /// Specifies the inode for this file. Can be [`fio::INO_UNKNOWN`] if not required.
    inode: u64,

    /// Vmo that backs the file.
    vmo: Vmo,
}

unsafe impl Sync for VmoFile {}

impl VmoFile {
    /// Create a new VmoFile which is backed by an existing Vmo.
    ///
    /// # Arguments
    ///
    /// * `vmo` - Vmo backing this file object.
    pub fn new(vmo: zx::Vmo) -> Arc<Self> {
        Self::new_with_inode(vmo, fio::INO_UNKNOWN)
    }

    /// Create a new VmoFile with a specified inode value.
    ///
    /// # Arguments
    ///
    /// * `vmo` - Vmo backing this file object.
    /// * `inode` - Inode value to report when getting the VmoFile's attributes.
    pub fn new_with_inode(vmo: zx::Vmo, inode: u64) -> Arc<Self> {
        Self::new_with_inode_and_executable(vmo, inode, false)
    }

    /// Create a new VmoFile with specified inode and executable values.
    ///
    /// # Arguments
    ///
    /// * `vmo` - Vmo backing this file object.
    /// * `inode` - Inode value to report when getting the VmoFile's attributes.
    /// * `executable` - If true, allow connections with OpenFlags::PERM_EXECUTE.
    pub fn new_with_inode_and_executable(vmo: zx::Vmo, inode: u64, executable: bool) -> Arc<Self> {
        Arc::new(VmoFile { executable, inode, vmo })
    }

    /// Creates a `VmoFile` with the specified `data`.
    pub fn from_data(data: impl AsRef<[u8]>) -> Arc<Self> {
        Self::from_bytes(data.as_ref())
    }

    fn from_bytes(bytes: &[u8]) -> Arc<Self> {
        let vmo = Vmo::create(bytes.len().try_into().unwrap()).unwrap();
        if !bytes.is_empty() {
            vmo.write(bytes, 0).unwrap();
        }
        Self::new(vmo)
    }
}

impl FileLike for VmoFile {
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        options: FileOptions,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        StreamIoConnection::create_sync(scope, self, options, object_request.take());
        Ok(())
    }
}

impl GetEntryInfo for VmoFile {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(self.inode, fio::DirentType::File)
    }
}

impl DirectoryEntry for VmoFile {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_file(self)
    }
}

impl Node for VmoFile {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        let content_size = if requested_attributes.intersects(
            fio::NodeAttributesQuery::CONTENT_SIZE.union(fio::NodeAttributesQuery::STORAGE_SIZE),
        ) {
            Some(self.vmo.get_content_size()?)
        } else {
            None
        };
        let mut abilities = fio::Operations::GET_ATTRIBUTES | fio::Operations::READ_BYTES;
        if self.executable {
            abilities |= fio::Operations::EXECUTE
        }
        Ok(immutable_attributes!(
            requested_attributes,
            Immutable {
                protocols: fio::NodeProtocolKinds::FILE,
                abilities: abilities,
                content_size: content_size,
                storage_size: content_size,
                id: self.inode,
            }
        ))
    }
}

// Required by `StreamIoConnection`.
impl GetVmo for VmoFile {
    fn get_vmo(&self) -> &zx::Vmo {
        &self.vmo
    }
}

impl File for VmoFile {
    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        false
    }

    fn executable(&self) -> bool {
        self.executable
    }

    async fn open_file(&self, _options: &FileOptions) -> Result<(), Status> {
        Ok(())
    }

    async fn truncate(&self, _length: u64) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn get_backing_memory(&self, flags: fio::VmoFlags) -> Result<zx::Vmo, Status> {
        // Logic here matches fuchsia.io requirements and matches what works for memfs.
        // Shared requests are satisfied by duplicating an handle, and private shares are
        // child VMOs.
        let vmo_rights = vmo_flags_to_rights(flags)
            | zx::Rights::BASIC
            | zx::Rights::MAP
            | zx::Rights::GET_PROPERTY;
        // Unless private sharing mode is specified, we always default to shared.
        if flags.contains(fio::VmoFlags::PRIVATE_CLONE) {
            get_as_private(&self.vmo, vmo_rights)
        } else {
            self.vmo.duplicate_handle(vmo_rights)
        }
    }

    async fn get_size(&self) -> Result<u64, Status> {
        Ok(self.vmo.get_content_size()?)
    }

    async fn update_attributes(
        &self,
        _attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn sync(&self, _mode: SyncMode) -> Result<(), Status> {
        Ok(())
    }
}

fn get_as_private(vmo: &zx::Vmo, mut rights: zx::Rights) -> Result<zx::Vmo, zx::Status> {
    // SNAPSHOT_AT_LEAST_ON_WRITE removes ZX_RIGHT_EXECUTE even if the parent VMO has it, adding
    // CHILD_NO_WRITE will ensure EXECUTE is maintained.
    const CHILD_OPTIONS: zx::VmoChildOptions =
        zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE.union(zx::VmoChildOptions::NO_WRITE);

    // Allow for the child VMO's content size and name to be changed.
    rights |= zx::Rights::SET_PROPERTY;

    let size = vmo.get_content_size()?;
    let new_vmo = vmo.create_child(CHILD_OPTIONS, 0, size)?;
    new_vmo.replace_handle(rights)
}
