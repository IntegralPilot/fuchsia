// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::binder::BinderDevice;
use crate::remote_binder::RemoteBinderDevice;
use starnix_core::device::DeviceOps;
use starnix_core::mm::MemoryAccessorExt;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::pseudo::simple_file::BytesFile;
use starnix_core::vfs::pseudo::vec_directory::{VecDirectory, VecDirectoryEntry};
use starnix_core::vfs::{
    CacheMode, DirEntry, DirectoryEntryType, FileObject, FileOps, FileSystem, FileSystemHandle,
    FileSystemOps, FileSystemOptions, FsNode, FsNodeHandle, FsNodeInfo, FsNodeOps, FsStr, FsString,
    NamespaceNode, SpecialNode, fileops_impl_dataless, fileops_impl_nonseekable,
    fileops_impl_noop_sync, fs_node_impl_dir_readonly,
};
use starnix_sync::{BinderFsDevicesLevel, LockDepMutex};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_types::vfs::default_statfs;
use starnix_uapi::auth::FsCred;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::{Errno, error};
use starnix_uapi::file_mode::mode;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::{BINDERFS_SUPER_MAGIC, statfs, uapi};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::sync::Arc;

pub struct BinderFs;
impl FileSystemOps for BinderFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(BINDERFS_SUPER_MAGIC))
    }
    fn name(&self) -> &'static FsStr {
        "binder".into()
    }
}

const DEFAULT_BINDERS: [&str; 3] = ["binder", "hwbinder", "vndbinder"];
const FEATURES_DIR: &str = "features";
const BINDER_CONTROL_DEVICE: &str = "binder-control";
// Binders with these names cannot be dynamically created using binder-control.
const RESERVED_NAMES: [&str; 2] = [FEATURES_DIR, BINDER_CONTROL_DEVICE];

#[derive(Debug)]
pub struct BinderFsDir {
    control_device: DeviceId,
    state: Arc<BinderFsState>,
}

#[derive(Debug)]
pub struct BinderFsState {
    devices: LockDepMutex<BTreeMap<FsString, DeviceId>, BinderFsDevicesLevel>,
}

impl BinderFsDir {
    pub fn new(kernel: &Kernel) -> Result<Self, Errno> {
        let registry = &kernel.device_registry;
        let mut devices = BTreeMap::<FsString, DeviceId>::default();
        let remote_device =
            registry.register_silent_dyn_device("remote-binder".into(), RemoteBinderDevice {})?;
        devices.insert("remote".into(), remote_device.devt);

        for name in DEFAULT_BINDERS {
            let device_metadata =
                registry.register_silent_dyn_device(name.into(), BinderDevice::default())?;
            devices.insert(name.into(), device_metadata.devt);
        }
        let state = Arc::new(BinderFsState { devices: devices.into() });

        let control_device = registry
            .register_silent_dyn_device(
                BINDER_CONTROL_DEVICE.into(),
                BinderControlDevice { state: state.clone() },
            )?
            .devt;

        Ok(Self { control_device, state })
    }
}

impl FsNodeOps for BinderFsDir {
    fs_node_impl_dir_readonly!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let mut entries = self
            .state
            .devices
            .lock()
            .keys()
            .map(|name| VecDirectoryEntry {
                entry_type: DirectoryEntryType::CHR,
                name: name.clone(),
                inode: None,
            })
            .collect::<Vec<_>>();
        entries.push(VecDirectoryEntry {
            entry_type: DirectoryEntryType::DIR,
            name: FEATURES_DIR.into(),
            inode: None,
        });
        entries.push(VecDirectoryEntry {
            entry_type: DirectoryEntryType::CHR,
            name: BINDER_CONTROL_DEVICE.into(),
            inode: None,
        });
        Ok(VecDirectory::new_file(entries))
    }

    fn lookup(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        if name == FEATURES_DIR {
            Ok(node.fs().create_node_and_allocate_node_id(
                BinderFeaturesDir::new(),
                FsNodeInfo::new(mode!(IFDIR, 0o755), FsCred::root()),
            ))
        } else if name == BINDER_CONTROL_DEVICE {
            let mut info = FsNodeInfo::new(mode!(IFCHR, 0o600), FsCred::root());
            info.rdev = self.control_device;
            Ok(node.fs().create_node_and_allocate_node_id(SpecialNode, info))
        } else if let Some(dev) = self.state.devices.lock().get(name) {
            let mode = if name == "remote" { mode!(IFCHR, 0o444) } else { mode!(IFCHR, 0o600) };
            let mut info = FsNodeInfo::new(mode, FsCred::root());
            info.rdev = *dev;
            Ok(node.fs().create_node_and_allocate_node_id(SpecialNode, info))
        } else {
            error!(ENOENT, format!("looking for {name}"))
        }
    }
}

impl BinderFs {
    pub fn new_fs(
        current_task: &CurrentTask,
        options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        let kernel = current_task.kernel();
        let fs = FileSystem::new(kernel, CacheMode::Permanent, BinderFs, options)?;
        let ops = BinderFsDir::new(kernel)?;
        let root_ino = fs.allocate_ino();
        fs.create_root(root_ino, ops);
        Ok(fs)
    }
}

#[derive(Clone)]
struct BinderControlDevice {
    state: Arc<BinderFsState>,
}

impl DeviceOps for BinderControlDevice {
    fn open(
        &self,
        _current_task: &CurrentTask,
        _devt: DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(self.clone()))
    }
}

impl FileOps for BinderControlDevice {
    fileops_impl_dataless!();
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        if request == uapi::BINDER_CTL_ADD {
            let user_arg = UserAddress::from(arg);
            if user_arg.is_null() {
                return error!(EINVAL);
            }
            let user_ref = UserRef::<uapi::binderfs_device>::new(user_arg);
            let mut request = current_task.read_object(user_ref)?;
            let name: Vec<u8> =
                request.name.iter().copied().map(|x| x as u8).take_while(|x| *x != 0).collect();
            // Invalid names return EACCES.
            if DirEntry::is_reserved_name((*name).into()) {
                return error!(EACCES);
            }
            if name.contains(&('/' as u8)) {
                return error!(EACCES);
            }
            // Names of already-existing objects return EEXIST.
            for reserved_name in RESERVED_NAMES {
                if *name == *reserved_name.as_bytes() {
                    return error!(EEXIST);
                }
            }
            let mut devices = self.state.devices.lock();
            match devices.entry(FsString::from(name.clone())) {
                Entry::Occupied(_) => error!(EEXIST),
                Entry::Vacant(entry) => {
                    let kernel = current_task.kernel();
                    let device_metadata = kernel
                        .device_registry
                        .register_silent_dyn_device((*name).into(), BinderDevice::default())?;
                    entry.insert(device_metadata.devt);
                    request.major = device_metadata.devt.major();
                    request.minor = device_metadata.devt.minor();
                    current_task.write_object(user_ref, &request)?;
                    Ok(SUCCESS)
                }
            }
        } else {
            error!(EINVAL)
        }
    }
}

struct BinderFeaturesDir {
    features: BTreeMap<FsString, bool>,
}

impl BinderFeaturesDir {
    fn new() -> Self {
        Self { features: BTreeMap::from([("freeze_notification".into(), true)]) }
    }
}

impl FsNodeOps for BinderFeaturesDir {
    fs_node_impl_dir_readonly!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let entries = self
            .features
            .keys()
            .map(|name| VecDirectoryEntry {
                entry_type: DirectoryEntryType::REG,
                name: name.clone(),
                inode: None,
            })
            .collect::<Vec<_>>();
        Ok(VecDirectory::new_file(entries))
    }

    fn lookup(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        if let Some(enable) = self.features.get(name) {
            return Ok(node.fs().create_node_and_allocate_node_id(
                BytesFile::new_node(if *enable { b"1\n" } else { b"0\n" }.to_vec()),
                FsNodeInfo::new(mode!(IFREG, 0o444), FsCred::root()),
            ));
        }
        error!(ENOENT, format!("looking for {name}"))
    }
}
