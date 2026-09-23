// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This supports paging for ext4 files.  `zx_pager_supply_pages` requires is to transfer pages to
//! the target, hence the need for a transfer VMO.  This also uses a static zeroed VMO to transfer
//! pages that should be zeroed.

use fidl_fuchsia_starnix_runner::{
    ManagerCreatePagerRequest, ManagerMarker, PagerMarker, PagerRegisterFileRequest,
    PagerRegisterFileResponse, PagerSynchronousProxy,
};
use fuchsia_component::client::connect_to_protocol_sync;
use starnix_core::vfs::FsStr;
use starnix_uapi::error;
use starnix_uapi::errors::Errno;
use std::ops::Range;

/// A struct representing a Pager instance running in the starnix_runner.
///
/// The actual pager logic lives outside of the starnix_kernel in order to avoid issues when the
/// container is suspended. Specifically, if the pager threads are suspended before their client
/// threads the clients may become blocked and unsuspendable. This means that the container suspend
/// operation will deadlock.
pub struct Pager {
    pager: PagerSynchronousProxy,
}

/// A single extent.
pub struct PagerExtent {
    pub logical: Range<u32>,
    pub physical_block: u64,
}

impl Pager {
    /// Returns a new pager.  `block_size` shouldn't be too big (which might cause overflows) and it
    /// should be a power of 2.
    pub fn new(backing_vmo: zx::Vmo, block_size: u64) -> Result<Self, Errno> {
        if block_size > 1024 * 1024 || !block_size.is_power_of_two() {
            return error!(EINVAL, "Bad block size {block_size}");
        }

        let manager =
            connect_to_protocol_sync::<ManagerMarker>().expect("Failed to connect to pager");
        let (pager, pager_server) = fidl::endpoints::create_sync_proxy::<PagerMarker>();
        manager
            .create_pager(ManagerCreatePagerRequest {
                backing_vmo: Some(backing_vmo),
                block_size: Some(block_size),
                pager: Some(pager_server),
                ..Default::default()
            })
            .expect("Failed to create pager");

        Ok(Self { pager })
    }

    /// Registers the file with the pager.  Returns a child VMO.  `extents` should be sorted.
    pub fn register(
        &self,
        name: &FsStr,
        inode_num: u32,
        size: u64,
        extents: &[PagerExtent],
    ) -> Result<zx::Vmo, zx::Status> {
        match self.pager.register_file(
            &PagerRegisterFileRequest {
                name: Some(name.to_string()),
                inode_num: Some(inode_num),
                size: Some(size),
                extents: Some(
                    extents
                        .iter()
                        .map(|e| fidl_fuchsia_starnix_runner::PagerExtent {
                            logical_start: e.logical.start,
                            logical_end: e.logical.end,
                            physical_block: e.physical_block,
                        })
                        .collect(),
                ),
                ..Default::default()
            },
            zx::Instant::INFINITE,
        ) {
            Ok(Ok(PagerRegisterFileResponse { vmo: Some(vmo), .. })) => Ok(vmo),
            Ok(Ok(_)) => Err(zx::Status::INTERNAL),
            Ok(Err(e)) => Err(zx::Status::err_from_raw(e)),
            Err(_) => Err(zx::Status::INTERNAL),
        }
    }
}
