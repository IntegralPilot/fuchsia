// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::proc_directory::ProcDirectory;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::{
    CacheMode, FileSystem, FileSystemHandle, FileSystemOps, FileSystemOptions, FsStr,
};

use starnix_types::vfs::default_statfs;
use starnix_uapi::errors::Errno;
use starnix_uapi::{PROC_SUPER_MAGIC, statfs};

struct ProcFsHandle(FileSystemHandle);

/// Returns `kernel`'s procfs instance, initializing it if needed.
pub fn proc_fs(
    current_task: &CurrentTask,
    options: FileSystemOptions,
) -> Result<FileSystemHandle, Errno> {
    Ok(current_task
        .kernel()
        .expando
        .get_or_init(|| ProcFsHandle(ProcFs::new_fs(current_task, options)))
        .0
        .clone())
}

/// `ProcFs` is a filesystem that exposes runtime information about a `Kernel` instance.
#[derive(Debug, Clone)]
struct ProcFs;

impl FileSystemOps for ProcFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(PROC_SUPER_MAGIC))
    }
    fn name(&self) -> &'static FsStr {
        "proc".into()
    }
}

impl ProcFs {
    /// Creates a new instance of `ProcFs` for the given `kernel`.
    pub fn new_fs(current_task: &CurrentTask, options: FileSystemOptions) -> FileSystemHandle {
        let kernel = current_task.kernel();
        let fs = FileSystem::new(kernel, CacheMode::Uncached, ProcFs, options)
            .expect("procfs constructed with valid options");
        let root_ino = fs.allocate_ino();
        fs.create_root(root_ino, ProcDirectory::new(kernel, &fs));
        fs
    }
}
