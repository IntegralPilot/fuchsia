// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ext4_lib::structs::INode;
use fidl_fuchsia_io as fio;

/// Representation of ext4 inode attributes.
///
/// Unlike [`INode`], which contains the full set of attributes as they are read from the
/// filesystem, and [`fio::NodeAttributes2`], which contains the full set of Fuchsia attributes,
/// this structure only stores the attributes that are supported by this implementation.
#[derive(Debug, Default)]
pub struct ExtAttributes {
    /// Access mode bits, corresponding to `i_mode`.
    pub mode: u16,

    /// Owner user ID.
    pub uid: u32,

    /// Owner group ID.
    pub gid: u32,
}

impl ExtAttributes {
    /// Creates attributes from an [`INode`].
    pub fn from_inode(inode: INode) -> Self {
        let mode = u16::from(inode.e2di_mode);

        Self { mode, uid: inode.e2di_uid.into(), gid: inode.e2di_gid.into() }
    }

    /// Returns a new [`fio::NodeAttributes2`] based on `attributes`, overwriting any of the
    /// attributes in `requested_attributes` with the value in `self`.
    pub fn overlay_node_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
        mut attributes: fio::NodeAttributes2,
    ) -> fio::NodeAttributes2 {
        if requested_attributes.contains(fio::NodeAttributesQuery::MODE) {
            attributes.mutable_attributes.mode = Some(u32::from(self.mode));
        }
        if requested_attributes.contains(fio::NodeAttributesQuery::UID) {
            attributes.mutable_attributes.uid = Some(self.uid);
        }
        if requested_attributes.contains(fio::NodeAttributesQuery::GID) {
            attributes.mutable_attributes.gid = Some(self.gid);
        }
        attributes
    }
}
