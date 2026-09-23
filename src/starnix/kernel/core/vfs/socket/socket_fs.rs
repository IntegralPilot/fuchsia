// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::{CurrentTask, Kernel};
use crate::vfs::{
    CacheMode, FileSystem, FileSystemHandle, FileSystemOps, FileSystemOptions, FsStr,
};
use starnix_types::vfs::default_statfs;
use starnix_uapi::errors::Errno;
use starnix_uapi::{SOCKFS_MAGIC, statfs};

/// `SocketFs` is the file system where anonymous socket nodes are created, for example in
/// `sys_socket`.
pub struct SocketFs;
impl FileSystemOps for SocketFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(SOCKFS_MAGIC))
    }
    fn name(&self) -> &'static FsStr {
        "sockfs".into()
    }
}

/// Returns a handle to the `SocketFs` instance in `kernel`, initializing it if needed.
pub fn socket_fs(kernel: &Kernel) -> FileSystemHandle {
    struct SocketFsHandle(FileSystemHandle);

    kernel
        .expando
        .get_or_init(|| {
            SocketFsHandle(
                FileSystem::new(
                    kernel,
                    CacheMode::Uncached,
                    SocketFs,
                    FileSystemOptions::default(),
                )
                .expect("socketfs constructed with valid options"),
            )
        })
        .0
        .clone()
}
