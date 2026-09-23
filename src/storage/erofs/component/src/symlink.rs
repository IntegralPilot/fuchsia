// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::volume::ErofsVolume;
use erofs::SymlinkNode;
use fidl_fuchsia_io as fio;
use std::sync::Arc;
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::node::Node;
use vfs::symlink::Symlink;

/// An implementation of an EROFS symlink node.
pub struct ErofsSymlink {
    volume: Arc<ErofsVolume>,
    node: SymlinkNode,
    target: Vec<u8>,
}

impl ErofsSymlink {
    pub fn new(volume: Arc<ErofsVolume>, node: SymlinkNode) -> Result<Arc<Self>, zx::Status> {
        let target = volume.fs().read_symlink(&node).map_err(|e| e.to_status())?;
        Ok(Arc::new(Self { volume, node, target }))
    }

    pub fn node(&self) -> &SymlinkNode {
        &self.node
    }
}

impl GetEntryInfo for ErofsSymlink {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(self.node.nid(), fio::DirentType::Symlink)
    }
}

impl DirectoryEntry for ErofsSymlink {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), zx::Status> {
        request.open_symlink(self)
    }
}

impl Node for ErofsSymlink {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, zx::Status> {
        let mtime = self.node.mtime_ns();
        let content_size = self.target.len() as u64;
        let selinux_context = self
            .volume
            .fs()
            .get_xattr(&self.node, b"security.selinux")
            .ok()
            .flatten()
            .map(fio::SelinuxContext::Data);
        Ok(vfs::attributes!(
            requested_attributes,
            Mutable {
                mode: self.node.mode() as u32,
                uid: self.node.uid(),
                gid: self.node.gid(),
                creation_time: mtime,
                modification_time: mtime,
                access_time: mtime,
                selinux_context: selinux_context,
            },
            Immutable {
                protocols: fio::NodeProtocolKinds::SYMLINK,
                abilities: fio::Operations::GET_ATTRIBUTES,
                content_size: content_size,
                storage_size: content_size,
                id: self.node.nid(),
                link_count: self.node.link_count() as u64,
                change_time: mtime,
            }
        ))
    }

    async fn list_extended_attributes(&self) -> Result<Vec<Vec<u8>>, zx::Status> {
        self.volume.fs().list_xattrs(&self.node).map_err(|e| e.to_status())
    }

    async fn get_extended_attribute(&self, name: Vec<u8>) -> Result<Vec<u8>, zx::Status> {
        self.volume
            .fs()
            .get_xattr(&self.node, &name)
            .map_err(|e| e.to_status())?
            .ok_or(zx::Status::NOT_FOUND)
    }
}

impl Symlink for ErofsSymlink {
    async fn read_target(&self) -> Result<Vec<u8>, zx::Status> {
        Ok(self.target.clone())
    }
}
