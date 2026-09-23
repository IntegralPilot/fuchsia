// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::task::CurrentTask;
use starnix_core::vfs::{FsNode, FsNodeOps, SymlinkTarget, fs_node_impl_symlink};

use starnix_uapi::errors::Errno;

/// A node that represents a link to /sys/firmware/devicetree/base
pub struct DeviceTreeSymlink;

impl DeviceTreeSymlink {
    pub fn new_node() -> impl FsNodeOps {
        Self {}
    }
}

impl FsNodeOps for DeviceTreeSymlink {
    fs_node_impl_symlink!();

    fn readlink(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
    ) -> Result<SymlinkTarget, Errno> {
        Ok(SymlinkTarget::Path("/sys/firmware/devicetree/base".into()))
    }
}
