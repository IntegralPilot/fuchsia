// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::security;
use crate::task::{CurrentTask, Kernel};
use crate::vfs::{
    CacheMode, DirEntry, FileHandle, FileObject, FileOps, FileSystem, FileSystemHandle,
    FileSystemOps, FileSystemOptions, FsNode, FsNodeFlags, FsNodeHandle, FsNodeInfo, FsNodeOps,
    FsStr, NamespaceNode, fs_node_impl_not_dir,
};
use starnix_types::vfs::default_statfs;
use starnix_uapi::auth::FsCred;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{ANON_INODE_FS_MAGIC, error, statfs};

pub struct Anon {}

impl FsNodeOps for Anon {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        error!(ENOSYS)
    }
}

impl Anon {
    /// Returns a new `Anon` instance for use in a binder device FD.
    pub fn new_for_binder_device() -> Self {
        Self {}
    }

    /// Returns a new `Anon` instance for use as the `FsNodeOps` of a socket.
    pub fn new_for_socket() -> Self {
        Self {}
    }

    /// Returns a new anonymous file with the specified properties, and a unique `FsNode`.
    pub fn new_file_extended(
        current_task: &CurrentTask,
        ops: Box<dyn FileOps>,
        flags: OpenFlags,
        name: &'static str,
        info: FsNodeInfo,
    ) -> Result<FileHandle, Errno> {
        Self::new_file_internal(current_task, ops, flags, name, info, FsNodeFlags::empty())
    }

    /// Returns a new anonymous file with the specified properties, and a unique `FsNode`.
    pub fn new_file(
        current_task: &CurrentTask,
        ops: Box<dyn FileOps>,
        flags: OpenFlags,
        name: &'static str,
    ) -> Result<FileHandle, Errno> {
        Self::new_file_extended(
            current_task,
            ops,
            flags,
            name,
            FsNodeInfo::new(FileMode::from_bits(0o600), current_task.current_fscred()),
        )
    }

    /// Returns a new anonymous file backed by a single "private" `FsNode`, to which no security
    /// labeling nor access-checks will be applied.
    pub fn new_private_file(
        current_task: &CurrentTask,
        ops: Box<dyn FileOps>,
        flags: OpenFlags,
        name: &'static str,
    ) -> FileHandle {
        let node = shared_private_node(current_task);
        security::fs_node_init_anon(current_task, &node, name)
            .expect("Private anon_inode creation cannot fail");
        let name = NamespaceNode::new_anonymous(DirEntry::new(node, None, name.into()));
        FileObject::new(current_task, ops, name, flags).unwrap()
    }

    /// Returns a new private anonymous file, applying caller-supplied `info`.
    // TODO: https://fxbug.dev/407611229 - Migrate callers off this and remove it.
    pub fn new_private_file_extended(
        current_task: &CurrentTask,
        ops: Box<dyn FileOps>,
        flags: OpenFlags,
        name: &'static str,
        info: FsNodeInfo,
    ) -> FileHandle {
        Self::new_file_internal(current_task, ops, flags, name, info, FsNodeFlags::IS_PRIVATE)
            .expect("Private anon_inode creation cannot fail")
    }

    fn new_file_internal(
        current_task: &CurrentTask,
        ops: Box<dyn FileOps>,
        flags: OpenFlags,
        name: &'static str,
        info: FsNodeInfo,
        node_flags: FsNodeFlags,
    ) -> Result<FileHandle, Errno> {
        let fs = anon_fs(current_task.kernel());
        let node = fs.create_node_with_flags(None, Anon {}, info, node_flags);
        security::fs_node_init_anon(current_task, &node, name)?;
        let name = NamespaceNode::new_anonymous(DirEntry::new(node, None, name.into()));
        FileObject::new(current_task, ops, name, flags)
    }
}

struct AnonFs;
impl FileSystemOps for AnonFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(ANON_INODE_FS_MAGIC))
    }
    fn name(&self) -> &'static FsStr {
        "anon_inodefs".into()
    }
}

pub fn anon_fs(kernel: &Kernel) -> FileSystemHandle {
    struct AnonFsHandle(FileSystemHandle);

    kernel
        .expando
        .get_or_init(|| {
            let fs =
                FileSystem::new(kernel, CacheMode::Uncached, AnonFs, FileSystemOptions::default())
                    .expect("anonfs constructed with valid options");
            AnonFsHandle(fs)
        })
        .0
        .clone()
}

fn shared_private_node(current_task: &CurrentTask) -> FsNodeHandle {
    struct CommonAnonFsNodeHandle(FsNodeHandle);

    let fs = anon_fs(current_task.kernel());

    current_task
        .kernel()
        .expando
        .get_or_init(|| {
            let info = FsNodeInfo::new(FileMode::from_bits(0o600), FsCred::root());
            let node = fs.create_node_with_flags(None, Anon {}, info, FsNodeFlags::IS_PRIVATE);
            CommonAnonFsNodeHandle(node)
        })
        .0
        .clone()
}
