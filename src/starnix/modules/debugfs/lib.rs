// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "512"]

use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::pseudo::simple_directory::{SimpleDirectory, SimpleDirectoryMutator};
use starnix_core::vfs::pseudo::stub_empty_file::StubEmptyFile;
use starnix_core::vfs::{
    CacheMode, FileSystem, FileSystemHandle, FileSystemOps, FileSystemOptions, FsStr,
};
use starnix_logging::bug_ref;
use starnix_types::vfs::default_statfs;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use starnix_uapi::{DEBUGFS_MAGIC, statfs};

struct DebugFs;

impl FileSystemOps for DebugFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(DEBUGFS_MAGIC))
    }
    fn name(&self) -> &'static FsStr {
        "debugfs".into()
    }
}

impl DebugFs {
    fn new_fs(kernel: &Kernel, options: FileSystemOptions) -> FileSystemHandle {
        let fs =
            FileSystem::new(kernel, CacheMode::Cached(kernel.fs_cache_config()), DebugFs, options)
                .expect("debugfs constructed with valid options");

        let root = SimpleDirectory::new();
        fs.create_root(fs.allocate_ino(), root.clone());

        let dir = SimpleDirectoryMutator::new(fs.clone(), root);
        let dir_mode = 0o700;
        dir.subdir("binder", dir_mode, |dir| {
            dir.entry(
                "failed_transaction_log",
                StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                mode!(IFREG, 0o444),
            );
            dir.entry(
                "state",
                StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                mode!(IFREG, 0o444),
            );
            dir.entry(
                "stats",
                StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                mode!(IFREG, 0o444),
            );
            dir.entry(
                "transaction_log",
                StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                mode!(IFREG, 0o444),
            );
            dir.entry(
                "transactions",
                StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                mode!(IFREG, 0o444),
            );
        });
        dir.subdir("mmc0", dir_mode, |dir| {
            dir.subdir("mmc0:0001", dir_mode, |dir| {
                dir.entry(
                    "ext_csd",
                    StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                    mode!(IFREG, 0o444),
                );
            });
        });
        dir.subdir("tracing", 0o644, |_| ());

        fs
    }
}

struct DebugFsHandle(FileSystemHandle);

pub fn debug_fs(
    current_task: &CurrentTask,
    _options: FileSystemOptions,
) -> Result<FileSystemHandle, Errno> {
    Ok(get_debugfs(current_task.kernel()))
}

pub fn get_debugfs(kernel: &Kernel) -> FileSystemHandle {
    kernel
        .expando
        .get_or_init(|| DebugFsHandle(DebugFs::new_fs(kernel, FileSystemOptions::default())))
        .0
        .clone()
}
