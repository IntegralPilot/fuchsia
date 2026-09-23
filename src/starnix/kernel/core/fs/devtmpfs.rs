// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::DeviceMode;
use crate::device::kobject::DeviceMetadata;
use crate::fs::tmpfs::{TmpFs, TmpFsData, TmpFsNodeType};
use crate::security;
use crate::task::dynamic_thread_spawner::SpawnRequestBuilder;
use crate::task::{CurrentTask, Kernel};
use crate::vfs::{
    DirEntryHandle, FileSystemHandle, FileSystemOptions, FsStr, LookupContext, MountInfo,
    NamespaceNode, path,
};
use starnix_uapi::auth::FsCred;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use std::collections::BTreeMap;

pub fn dev_tmp_fs(
    current_task: &CurrentTask,
    _options: FileSystemOptions,
) -> Result<FileSystemHandle, Errno> {
    Ok(DevTmpFs::from_kernel(current_task.kernel()))
}

pub struct DevTmpFs(());

impl DevTmpFs {
    pub fn from_kernel(kernel: &Kernel) -> FileSystemHandle {
        struct DevTmpFsHandle(FileSystemHandle);

        kernel.expando.get_or_init(|| DevTmpFsHandle(Self::init(kernel))).0.clone()
    }

    fn init(kernel: &Kernel) -> FileSystemHandle {
        let fs = TmpFs::new_fs_with_name(kernel, "devtmpfs".into());

        let initial_content = TmpFsData {
            owner: FsCred::root(),
            perm: 0o755,
            node_type: TmpFsNodeType::Directory(BTreeMap::from([
                (
                    "shm".into(),
                    TmpFsData {
                        owner: FsCred::root(),
                        perm: 0o755,
                        node_type: TmpFsNodeType::Directory(Default::default()),
                    },
                ),
                (
                    "pts".into(),
                    TmpFsData {
                        owner: FsCred::root(),
                        perm: 0o755,
                        node_type: TmpFsNodeType::Directory(Default::default()),
                    },
                ),
                (
                    "fd".into(),
                    TmpFsData {
                        owner: FsCred::root(),
                        perm: 0o777,
                        node_type: TmpFsNodeType::Link("/proc/self/fd".into()),
                    },
                ),
            ])),
        };
        TmpFs::set_initial_content(kernel, &fs, initial_content);

        fs
    }
}

/// Creates a device node in the devtmpfs filesystem.
///
/// This function executes the creation in a background thread and blocks the
/// calling thread until the device node is fully created. This blocking behavior
/// matches Linux.
pub fn devtmpfs_create_device(
    kernel: &Kernel,
    device_metadata: DeviceMetadata,
) -> Result<(), Errno> {
    let closure = move |current_task: &CurrentTask| {
        current_task.override_creds(security::creds_start_internal_operation(current_task), || {
            devtmpfs_create_device_internal(current_task, &device_metadata)
        })
    };
    let (result_fn, req) = SpawnRequestBuilder::new()
        .with_debug_name("devtmpfs-create-device")
        .with_sync_closure(closure)
        .build_with_sync_result();
    kernel.kthreads.spawner().spawn_from_request(req);
    result_fn()?
}

fn devtmpfs_create_device_internal(
    current_task: &CurrentTask,
    device_metadata: &DeviceMetadata,
) -> Result<(), Errno> {
    let separator_pos = device_metadata.devname.iter().rposition(|&c| c == path::SEPARATOR);
    let (device_path, device_name) = match separator_pos {
        Some(pos) => device_metadata.devname.split_at(pos + 1),
        None => (&[] as &[u8], device_metadata.devname.as_slice()),
    };
    let parent_dir = device_path
        .split(|&c| c == path::SEPARATOR)
        // Avoid EEXIST for 'foo//bar' and the last directory name.
        .filter(|dir_name| dir_name.len() > 0)
        .try_fold(
            DevTmpFs::from_kernel(current_task.kernel()).root().clone(),
            |parent_dir, dir_name| {
                devtmpfs_get_or_create_directory_at(current_task, parent_dir, dir_name.into())
            },
        )?;
    devtmpfs_create_device_node(
        current_task,
        parent_dir,
        device_name.into(),
        device_metadata.mode,
        device_metadata.devt,
    )
}

pub fn devtmpfs_get_or_create_directory_at(
    current_task: &CurrentTask,
    parent_dir: DirEntryHandle,
    dir_name: &FsStr,
) -> Result<DirEntryHandle, Errno> {
    parent_dir.get_or_create_entry(
        current_task,
        &MountInfo::detached(),
        dir_name,
        |dir, mount, name| {
            dir.create_node(
                current_task,
                mount,
                name,
                mode!(IFDIR, 0o755),
                DeviceId::NONE,
                FsCred::root(),
            )
        },
    )
}

fn devtmpfs_create_device_node(
    current_task: &CurrentTask,
    parent_dir: DirEntryHandle,
    device_name: &FsStr,
    device_mode: DeviceMode,
    devt: DeviceId,
) -> Result<(), Errno> {
    let mode = match device_mode {
        DeviceMode::Char => mode!(IFCHR, 0o666),
        DeviceMode::Block => mode!(IFBLK, 0o666),
    };
    // This creates content inside the temporary FS. This doesn't depend on the mount
    // information.
    parent_dir.create_entry(
        current_task,
        &MountInfo::detached(),
        device_name,
        |dir, mount, name| dir.create_node(current_task, mount, name, mode, devt, FsCred::root()),
    )?;
    Ok(())
}

pub fn devtmpfs_remove_path(current_task: &CurrentTask, path: &FsStr) -> Result<(), Errno> {
    let root_node =
        NamespaceNode::new_anonymous(DevTmpFs::from_kernel(current_task.kernel()).root().clone());
    let mut context = LookupContext::default();
    let (parent_node, device_name) = current_task.lookup_parent(&mut context, &root_node, path)?;
    parent_node.entry.remove_child(device_name.into(), &current_task.kernel().mounts);
    Ok(())
}
