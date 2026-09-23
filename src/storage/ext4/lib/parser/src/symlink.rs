// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ext4_lib::parser::XattrMap;
use ext4_lib::processor::Ext4Processor;
use fidl_fuchsia_io as fio;
use log::warn;
use std::sync::Arc;
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::immutable_attributes;
use vfs::node::Node;
use vfs::symlink::Symlink;
use zx::Status;

use crate::types::ExtAttributes;

/// An ext4 filesystem symbolic link node.
pub struct ExtSymlink {
    inode: u64,
    attributes: ExtAttributes,
    xattrs: XattrMap,
    target: Vec<u8>,
}

impl std::fmt::Debug for ExtSymlink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtSymlink")
            .field("inode", &self.inode)
            .field("attributes", &self.attributes)
            .field("xattrs", &self.xattrs)
            .field("target", &String::from_utf8_lossy(&self.target))
            .finish_non_exhaustive()
    }
}

impl ExtSymlink {
    /// Creates a new [`ExtSymlink`] with the given `inode`, `attributes`, `xattrs` and `target`.
    pub fn new(
        inode: u64,
        attributes: ExtAttributes,
        xattrs: XattrMap,
        target: Vec<u8>,
    ) -> Arc<Self> {
        Arc::new(Self { inode, attributes, xattrs, target })
    }

    /// Creates a new [`ExtSymlink`] from the processor with the given inode number.
    pub fn from_processor(processor: Arc<Ext4Processor>, ino: u32) -> Result<Arc<Self>, Status> {
        let inode = processor.inode(ino).map_err(|e| {
            warn!("failed to parse symlink inode: {e}");
            Status::IO_DATA_INTEGRITY
        })?;
        let attributes = ExtAttributes::from_inode(inode);
        let xattrs = processor.inode_xattrs(ino).map_err(|e| {
            warn!("failed to parse symlink xattrs: {e}");
            Status::IO_DATA_INTEGRITY
        })?;
        let target = processor.read_data(ino).map_err(|e| {
            warn!("failed to parse symlink data: {e}");
            Status::IO_DATA_INTEGRITY
        })?;

        Ok(Self::new(ino as u64, attributes, xattrs, target))
    }
}

impl GetEntryInfo for ExtSymlink {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(self.inode, fio::DirentType::Symlink)
    }
}

impl DirectoryEntry for ExtSymlink {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_symlink(self)
    }
}

impl Node for ExtSymlink {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        Ok(self.attributes.overlay_node_attributes(
            requested_attributes,
            immutable_attributes!(
                requested_attributes,
                Immutable {
                    protocols: fio::NodeProtocolKinds::SYMLINK,
                    abilities: fio::Operations::GET_ATTRIBUTES,
                    content_size: self.target.len() as u64,
                    storage_size: self.target.len() as u64,
                    id: self.inode,
                }
            ),
        ))
    }

    async fn list_extended_attributes(&self) -> Result<Vec<Vec<u8>>, Status> {
        Ok(self.xattrs.keys().map(Clone::clone).collect())
    }

    async fn get_extended_attribute(&self, name: Vec<u8>) -> Result<Vec<u8>, Status> {
        self.xattrs.get(&name).map(Clone::clone).ok_or(Status::NOT_FOUND)
    }
}

impl Symlink for ExtSymlink {
    async fn read_target(&self) -> Result<Vec<u8>, Status> {
        Ok(self.target.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn test_read_target() {
        let expected_target = b"target_path";
        let symlink = ExtSymlink::new(
            123,
            ExtAttributes::default(),
            XattrMap::default(),
            expected_target.to_vec(),
        );

        assert_eq!(symlink.read_target().await.unwrap(), expected_target);
    }

    #[fuchsia::test]
    async fn test_get_attributes() {
        let target = b"some_target";
        let symlink = ExtSymlink::new(
            123,
            ExtAttributes { mode: 0o120755, uid: 456, gid: 789 },
            XattrMap::default(),
            target.to_vec(),
        );

        let attributes_query = fio::NodeAttributesQuery::ID
            | fio::NodeAttributesQuery::MODE
            | fio::NodeAttributesQuery::UID
            | fio::NodeAttributesQuery::GID
            | fio::NodeAttributesQuery::CONTENT_SIZE
            | fio::NodeAttributesQuery::STORAGE_SIZE;

        let attrs = symlink.get_attributes(attributes_query).await.unwrap();

        assert_eq!(attrs.immutable_attributes.id.unwrap(), 123);
        assert_eq!(attrs.mutable_attributes.mode.unwrap(), 0o120755);
        assert_eq!(attrs.mutable_attributes.uid.unwrap(), 456);
        assert_eq!(attrs.mutable_attributes.gid.unwrap(), 789);
        assert_eq!(attrs.immutable_attributes.content_size.unwrap(), target.len() as u64);
        assert_eq!(attrs.immutable_attributes.storage_size.unwrap(), target.len() as u64);
    }

    #[fuchsia::test]
    async fn test_extended_attributes() {
        let xattrs = [
            (b"user.attr1".to_vec(), b"value1".to_vec()),
            (b"user.attr2".to_vec(), b"value2".to_vec()),
        ]
        .into_iter()
        .collect();

        let symlink = ExtSymlink::new(123, ExtAttributes::default(), xattrs, b"target".to_vec());

        let mut keys = symlink.list_extended_attributes().await.unwrap();
        keys.sort();
        assert_eq!(keys, vec![b"user.attr1".to_vec(), b"user.attr2".to_vec()]);

        assert_eq!(
            symlink.get_extended_attribute(b"user.attr1".to_vec()).await.unwrap(),
            b"value1".to_vec()
        );
        assert_eq!(
            symlink.get_extended_attribute(b"user.attr2".to_vec()).await.unwrap(),
            b"value2".to_vec()
        );
        assert_eq!(
            symlink.get_extended_attribute(b"user.nonexistent".to_vec()).await.unwrap_err(),
            Status::NOT_FOUND
        );
    }
}
