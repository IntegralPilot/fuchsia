// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::security::{self, Auditable, PermissionFlags};
use starnix_core::task::CurrentTask;
use starnix_core::vfs::pseudo::simple_directory::{SimpleDirectory, SimpleDirectoryMutator};
use starnix_core::vfs::{
    AppendLockWriteGuard, CheckAccessReason, FileOps, FileSystemHandle, FsNode, FsNodeHandle,
    FsNodeInfo, FsNodeOps, fs_node_impl_not_dir,
};
use starnix_sync::DynamicLockDepRwLock;
use starnix_uapi::auth::Capabilities;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::{Access, FileMode};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{error, gid_t, uid_t};
use std::sync::Arc;

/// Wrapper around leaf [`FsNodeOps`] nodes under `/proc/sys` that enforces sysctl access-control
/// semantics.
///
/// Specifically:
/// * `CAP_DAC_OVERRIDE` does not grant write access to `/proc/sys` files.
/// * Non-owner processes holding `ADMIN_CAP` (`CAP_NET_ADMIN` under `/proc/sys/net`,
///   `CAP_SYS_ADMIN` elsewhere under `/proc/sys`) are granted the file owner's access permissions.
/// * Read-only files (`0444`) reject writes even from root (`uid == 0`) or processes with
///   administrative capabilities.
struct SysctlFsNodeOps<T: FsNodeOps, const ADMIN_CAP: u32> {
    inner: T,
}

impl<T: FsNodeOps, const ADMIN_CAP: u32> SysctlFsNodeOps<T, ADMIN_CAP> {
    fn new(inner: T) -> Self {
        Self { inner }
    }
}

impl<T: FsNodeOps, const ADMIN_CAP: u32> FsNodeOps for SysctlFsNodeOps<T, ADMIN_CAP> {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        node: &FsNode,
        current_task: &CurrentTask,
        flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        self.inner.create_file_ops(node, current_task, flags)
    }

    fn truncate(
        &self,
        guard: &AppendLockWriteGuard<'_>,
        node: &FsNode,
        current_task: &CurrentTask,
        length: u64,
    ) -> Result<(), Errno> {
        self.inner.truncate(guard, node, current_task, length)
    }

    fn check_access(
        &self,
        node: &FsNode,
        current_task: &CurrentTask,
        permission_flags: PermissionFlags,
        info: &DynamicLockDepRwLock<FsNodeInfo>,
        _reason: CheckAccessReason,
        audit_context: Auditable<'_>,
    ) -> Result<(), Errno> {
        let (node_uid, node_gid, node_mode) = {
            let info = info.read();
            (info.uid, info.gid, info.mode)
        };
        check_sysctl_access(
            current_task,
            permission_flags.as_access(),
            node_uid,
            node_gid,
            node_mode,
            Capabilities::from_bits_truncate(1 << ADMIN_CAP),
        )?;
        security::fs_node_permission(current_task, node, permission_flags, audit_context)
    }
}

fn check_sysctl_access(
    current_task: &CurrentTask,
    requested: Access,
    node_uid: uid_t,
    node_gid: gid_t,
    node_mode: FileMode,
    admin_capability: Capabilities,
) -> Result<(), Errno> {
    let (fsuid, is_in_group) = {
        let current_creds = current_task.current_creds();
        (current_creds.fsuid, current_creds.is_in_group(node_gid))
    };
    let granted = if fsuid == node_uid {
        node_mode.user_access()
    } else if is_in_group {
        node_mode.group_access()
    } else {
        node_mode.other_access()
    };

    let mut not_granted = requested;
    not_granted.remove(granted);
    if not_granted.is_empty() {
        return Ok(());
    }

    // Non-owner processes holding the directory's administrative capability (`CAP_NET_ADMIN` under
    // `/proc/sys/net` or `CAP_SYS_ADMIN` elsewhere under `/proc/sys`) are granted the owner's
    // access permissions (such as write access on 0644 files), whereas `CAP_DAC_OVERRIDE` is
    // ignored.
    if security::check_task_capable(current_task, admin_capability).is_ok() {
        not_granted.remove(node_mode.user_access());
    }

    if not_granted.is_empty() { Ok(()) } else { error!(EACCES) }
}

/// Directory wrapper around [`SimpleDirectory`] that enforces `/proc/sys` access controls on leaf
/// entries added via [`SysctlDirectory::edit`] or [`SysctlDirectoryMutator`].
pub(crate) struct SysctlDirectory<const ADMIN_CAP: u32> {
    directory: Arc<SimpleDirectory>,
}

impl<const ADMIN_CAP: u32> SysctlDirectory<ADMIN_CAP> {
    pub fn new() -> Self {
        Self { directory: SimpleDirectory::new() }
    }

    pub fn edit(
        &self,
        fs: &FileSystemHandle,
        callback: impl FnOnce(&SysctlDirectoryMutator<'_, ADMIN_CAP>),
    ) {
        let mutator = SysctlDirectoryMutator::new(fs.clone(), self);
        callback(&mutator);
    }

    pub fn into_node(self, fs: &FileSystemHandle, mode: u32) -> FsNodeHandle {
        self.directory.into_node(fs, mode)
    }
}

/// Builder wrapper around [`SimpleDirectoryMutator`] that automatically wraps leaf files in
/// [`SysctlFsNodeOps`] using the parent [`SysctlDirectory`]'s administrative capability.
pub(crate) struct SysctlDirectoryMutator<'a, const ADMIN_CAP: u32> {
    fs: FileSystemHandle,
    directory: &'a SysctlDirectory<ADMIN_CAP>,
    mutator: SimpleDirectoryMutator,
}

impl<'a, const ADMIN_CAP: u32> SysctlDirectoryMutator<'a, ADMIN_CAP> {
    pub fn new(fs: FileSystemHandle, directory: &'a SysctlDirectory<ADMIN_CAP>) -> Self {
        let mutator = SimpleDirectoryMutator::new(fs.clone(), directory.directory.clone());
        Self { fs, directory, mutator }
    }

    pub fn entry(&self, name: &str, ops: impl FsNodeOps, mode: FileMode) {
        if mode.is_dir() {
            self.mutator.entry(name, ops, mode);
        } else {
            self.mutator.entry(name, SysctlFsNodeOps::<_, ADMIN_CAP>::new(ops), mode);
        }
    }

    pub fn subdir(
        &self,
        name: &str,
        mode: u32,
        build_subdir: impl FnOnce(&SysctlDirectoryMutator<'_, ADMIN_CAP>),
    ) {
        self.subdir_with_capability::<ADMIN_CAP>(name, mode, build_subdir);
    }

    pub fn subdir_with_capability<const SUBDIR_CAP: u32>(
        &self,
        name: &str,
        mode: u32,
        build_subdir: impl FnOnce(&SysctlDirectoryMutator<'_, SUBDIR_CAP>),
    ) {
        let sub_dir = SysctlDirectory::<SUBDIR_CAP> {
            directory: self.directory.directory.subdir(&self.fs, name.into(), mode),
        };
        let sub_mutator = SysctlDirectoryMutator::new(self.fs.clone(), &sub_dir);
        build_subdir(&sub_mutator);
    }
}
