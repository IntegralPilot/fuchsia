// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::task::CurrentTask;
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::fs_registry::FsRegistry;
use starnix_core::vfs::pseudo::dynamic_file::{DynamicFile, DynamicFileBuf, DynamicFileSource};
use starnix_uapi::errors::Errno;
use starnix_uapi::fs_type::FileSystemTypeFlags;
use std::sync::Arc;

#[derive(Clone)]
pub struct FilesystemsFile {
    fs_registry: Arc<FsRegistry>,
}

impl FilesystemsFile {
    pub fn new_node(fs_registry: &Arc<FsRegistry>) -> impl FsNodeOps {
        DynamicFile::new_node(Self { fs_registry: fs_registry.clone() })
    }
}

impl DynamicFileSource for FilesystemsFile {
    fn generate(
        &self,
        _current_task: &CurrentTask,
        sink: &mut DynamicFileBuf,
    ) -> Result<(), Errno> {
        for (name, flags) in self.fs_registry.list_all() {
            let prefix =
                if flags.contains(FileSystemTypeFlags::REQUIRES_DEV) { "" } else { "nodev" };
            writeln!(sink, "{}\t{}", prefix, name)?;
        }

        Ok(())
    }
}
