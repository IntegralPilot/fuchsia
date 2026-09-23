// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::vfs::pseudo::simple_file::BytesFile;
use starnix_core::vfs::{FileSystemHandle, FsNodeHandle, FsNodeInfo};
use starnix_uapi::auth::FsCred;
use starnix_uapi::mode;

/// A node that is empty, used as placeholder.
pub fn cgroups_file(fs: &FileSystemHandle) -> FsNodeHandle {
    fs.create_node_and_allocate_node_id(
        BytesFile::new_node(vec![]),
        FsNodeInfo::new(mode!(IFREG, 0o444), FsCred::root()),
    )
}
