// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::directory::ErofsDirectory;
use crate::pager::ErofsPager;
use anyhow::Context as _;
use erofs::ErofsFilesystem;
use erofs::readers::VmoReader;
use fidl_fuchsia_io as fio;
use std::sync::Arc;
use vfs::execution_scope::ExecutionScope;

// An array used to initialize the FilesystemInfo |name| field. This just spells "erofs" 0-padded to
// 32 bytes.
const EROFS_INFO_NAME_FIDL: [i8; 32] = [
    0x65, 0x72, 0x6f, 0x66, 0x73, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0,
];

/// Holds the volume-level state for an active EROFS instance.
pub struct ErofsVolume {
    /// The filesystem for this EROFS volume.
    fs: ErofsFilesystem,
    /// Reference to the unified pager.
    pager: Arc<ErofsPager>,
    /// Unique filesystem ID.
    fs_id: u64,
}

impl ErofsVolume {
    pub fn new(backing_vmo: zx::Vmo, pager: Arc<ErofsPager>) -> Result<Self, anyhow::Error> {
        let fs_id = zx::Event::create().koid().context("Failed to get event koid")?.raw_koid();
        let reader =
            Arc::new(VmoReader::new(Arc::new(backing_vmo)).context("Failed to create VmoReader")?);
        let fs = ErofsFilesystem::new(reader).context("Failed to create ErofsFilesystem")?;
        Ok(Self { fs, pager, fs_id })
    }

    /// Sets up and serves an EROFS volume from a backing VMO.
    pub fn serve(
        backing_vmo: zx::Vmo,
        pager: Arc<ErofsPager>,
        flags: fio::Flags,
        root: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    ) -> Result<(), anyhow::Error> {
        let scope = ExecutionScope::new();
        let volume = Arc::new(Self::new(backing_vmo, pager)?);
        let root_node = volume.fs().root_node();
        let root_dir = Arc::new(ErofsDirectory::new(volume, root_node));

        vfs::directory::serve_on(root_dir, flags, scope, root);
        Ok(())
    }

    /// Returns a reference to the filesystem.
    pub fn fs(&self) -> &ErofsFilesystem {
        &self.fs
    }

    /// Returns a reference to the pager.
    pub fn pager(&self) -> &ErofsPager {
        &self.pager
    }

    pub fn query_filesystem(&self) -> Result<fio::FilesystemInfo, zx::Status> {
        let total_bytes = self.fs.total_bytes();
        let total_inodes = self.fs.total_inodes();
        let block_size = self.fs.block_size() as u32;

        Ok(fio::FilesystemInfo {
            total_bytes,
            used_bytes: total_bytes,
            total_nodes: total_inodes,
            used_nodes: total_inodes,
            free_shared_pool_bytes: 0,
            fs_id: self.fs_id,
            block_size,
            max_filename_size: 255,
            fs_type: fidl_fuchsia_fs::VfsType::Erofs.into_primitive(),
            padding: 0,
            name: EROFS_INFO_NAME_FIDL,
        })
    }
}
