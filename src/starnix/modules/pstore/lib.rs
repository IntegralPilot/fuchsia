// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "512"]

use bootreason::get_console_ramoops;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::pseudo::simple_directory::SimpleDirectory;
use starnix_core::vfs::pseudo::simple_file::BytesFile;
use starnix_core::vfs::{
    CacheMode, FileSystem, FileSystemHandle, FileSystemOps, FileSystemOptions, FsStr,
};

use starnix_types::vfs::default_statfs;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use starnix_uapi::{PSTOREFS_MAGIC, statfs};

struct PstoreFsHandle {
    fs_handle: FileSystemHandle,
}

pub fn pstore_fs(
    current_task: &CurrentTask,
    options: FileSystemOptions,
) -> Result<FileSystemHandle, Errno> {
    let handle = current_task.kernel().expando.get_or_try_init(|| {
        Ok(PstoreFsHandle { fs_handle: PstoreFs::new_fs(current_task, options)? })
    })?;
    Ok(handle.fs_handle.clone())
}

pub struct PstoreFs;

impl FileSystemOps for PstoreFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(PSTOREFS_MAGIC))
    }

    fn name(&self) -> &'static FsStr {
        "pstore".into()
    }
}

impl PstoreFs {
    pub fn new_fs(
        current_task: &CurrentTask,
        options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        let kernel = current_task.kernel();
        let fs = FileSystem::new(kernel, CacheMode::Permanent, PstoreFs, options)?;

        let dir = SimpleDirectory::new();
        dir.edit(&fs, |dir| {
            if let Some(ramoops_contents) = get_console_ramoops() {
                let ramoops_contents_0 = ramoops_contents.clone();
                dir.entry(
                    "console-ramoops-0",
                    BytesFile::new_node(ramoops_contents_0),
                    mode!(IFREG, 0o440),
                );
                dir.entry(
                    "console-ramoops",
                    BytesFile::new_node(ramoops_contents),
                    mode!(IFREG, 0o440),
                );
            }
        });

        let root_ino = fs.allocate_ino();
        fs.create_root(root_ino, dir);
        Ok(fs)
    }
}
