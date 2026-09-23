// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of a cgroup directory.
//!
//! Contains core cgroup interface files, resource controller interface files, and directories for
//! sub-cgroups.
//!
//! Full details at https://docs.kernel.org/admin-guide/cgroup-v2.html

use std::collections::BTreeMap;
use std::ops::Deref;
use std::sync::{Arc, Weak};

use starnix_core::task::{CgroupOps, CgroupRoot, CurrentTask};
use starnix_core::vfs::pseudo::simple_file::BytesFile;
use starnix_core::vfs::pseudo::stub_bytes_file::StubBytesFile;
use starnix_core::vfs::pseudo::vec_directory::{VecDirectory, VecDirectoryEntry};
use starnix_core::vfs::{
    DirectoryEntryType, FileOps, FileSystemHandle, FsNode, FsNodeHandle, FsNodeInfo, FsNodeOps,
    FsStr, FsString,
};
use starnix_logging::bug_ref;
use starnix_sync::{CgroupDirectoryInterfaceFilesLock, LockDepMutex};
use starnix_uapi::auth::FsCred;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{errno, error, mode};

use crate::DirectoryNodes;
use crate::cpuset::CpusetCpusFile;
use crate::events::EventsFile;
use crate::freeze::FreezeFile;
use crate::fs::CgroupVersion;
use crate::kill::KillFile;
use crate::procs::ControlGroupNode;

const CONTROLLERS_FILE: &str = "cgroup.controllers";
const PROCS_FILE: &str = "cgroup.procs";
const FREEZE_FILE: &str = "cgroup.freeze";
const EVENTS_FILE: &str = "cgroup.events";
const KILL_FILE: &str = "cgroup.kill";
const TYPE_FILE: &str = "cgroup.type";
const SUBTREE_CONTROL_FILE: &str = "cgroup.subtree_control";
const TASKS_FILE: &str = "tasks";
const CPUS_FILE: &str = "cpus";

#[derive(Debug)]
pub struct CgroupDirectory {
    /// The associated cgroup.
    cgroup: Weak<dyn CgroupOps>,

    /// Weak reference to all other directory nodes of the filesystem. Used to fetch subdirectories.
    dir_nodes: Weak<DirectoryNodes>,

    /// Interface files of the current cgroup directory. Files can be added or removed when resource
    /// controllers are enabled and disabled on the cgroup, respectively.
    interface_files:
        LockDepMutex<BTreeMap<FsString, FsNodeHandle>, CgroupDirectoryInterfaceFilesLock>,
}

impl CgroupDirectory {
    /// 2-step initialization to create a new root directory. `FileSystem` is instantiated with
    /// the `Directory`, which is then used to create the interface files of the `Directory` using
    /// `create_root_interface_files()`.
    pub fn new_root(
        cgroup: Weak<CgroupRoot>,
        dir_nodes: Weak<DirectoryNodes>,
    ) -> CgroupDirectoryHandle {
        CgroupDirectoryHandle(Arc::new(Self {
            cgroup: cgroup as Weak<dyn CgroupOps>,
            interface_files: Default::default(),
            dir_nodes,
        }))
    }

    /// Can only be called on a newly initialized root directory created by `new_root`, and can only
    /// be called once. Creates interface files for the root directory, which can only be done after
    /// the `FileSystem` is initialized.
    pub fn create_root_interface_files(&self, fs: &FileSystemHandle) {
        let mut interface_files = self.interface_files.lock();
        assert!(interface_files.is_empty(), "init is only called once");
        match self.version().unwrap() {
            CgroupVersion::V2 => {
                interface_files.insert(
                    PROCS_FILE.into(),
                    fs.create_node_and_allocate_node_id(
                        ControlGroupNode::new(self.cgroup.clone()),
                        FsNodeInfo::new(mode!(IFREG, 0o644), FsCred::root()),
                    ),
                );
                interface_files.insert(
                    CONTROLLERS_FILE.into(),
                    fs.create_node_and_allocate_node_id(
                        BytesFile::new_node(b"".to_vec()),
                        FsNodeInfo::new(mode!(IFREG, 0o444), FsCred::root()),
                    ),
                );
                interface_files.insert(
                    SUBTREE_CONTROL_FILE.into(),
                    fs.create_node_and_allocate_node_id(
                        BytesFile::new_node(b"".to_vec()),
                        FsNodeInfo::new(mode!(IFREG, 0o644), FsCred::root()),
                    ),
                );
            }
            CgroupVersion::V1(key) => {
                // TODO(https://fxbug.dev/401298305): "tasks" should contain task IDs (TIDs)
                // of all threads in the cgroup, and allow migrating individual threads. Currently
                // it uses `ControlGroupNode` which only supports process IDs (PIDs).
                interface_files.insert(
                    TASKS_FILE.into(),
                    fs.create_node_and_allocate_node_id(
                        ControlGroupNode::new(self.cgroup.clone()),
                        FsNodeInfo::new(mode!(IFREG, 0o644), FsCred::root()),
                    ),
                );
                interface_files.insert(
                    PROCS_FILE.into(),
                    fs.create_node_and_allocate_node_id(
                        ControlGroupNode::new(self.cgroup.clone()),
                        FsNodeInfo::new(mode!(IFREG, 0o644), FsCred::root()),
                    ),
                );
                if key.controllers.contains(&starnix_core::task::ControllerType::Cpuset) {
                    interface_files.insert(
                        CPUS_FILE.into(),
                        fs.create_node_and_allocate_node_id(
                            CpusetCpusFile::new_node(self.cgroup.clone()),
                            FsNodeInfo::new(mode!(IFREG, 0o644), FsCred::root()),
                        ),
                    );
                }
            }
        }
    }

    /// Creates a new non-root directory, along with its core interface files.
    pub(crate) fn new(
        cgroup: Weak<dyn CgroupOps>,
        fs: &FileSystemHandle,
        dir_nodes: &Arc<DirectoryNodes>,
        owner: FsCred,
    ) -> CgroupDirectoryHandle {
        let version = &dir_nodes.version;
        let mut interface_files = BTreeMap::new();
        match version {
            CgroupVersion::V2 => {
                interface_files = BTreeMap::from([
                    (
                        PROCS_FILE.into(),
                        fs.create_node_and_allocate_node_id(
                            ControlGroupNode::new(cgroup.clone()),
                            FsNodeInfo::new(mode!(IFREG, 0o644), owner.clone()),
                        ),
                    ),
                    (
                        CONTROLLERS_FILE.into(),
                        fs.create_node_and_allocate_node_id(
                            BytesFile::new_node(b"".to_vec()),
                            FsNodeInfo::new(mode!(IFREG, 0o444), owner.clone()),
                        ),
                    ),
                    (
                        FREEZE_FILE.into(),
                        fs.create_node_and_allocate_node_id(
                            FreezeFile::new_node(cgroup.clone()),
                            FsNodeInfo::new(mode!(IFREG, 0o644), owner.clone()),
                        ),
                    ),
                    (
                        EVENTS_FILE.into(),
                        fs.create_node_and_allocate_node_id(
                            EventsFile::new_node(cgroup.clone()),
                            FsNodeInfo::new(mode!(IFREG, 0o444), owner.clone()),
                        ),
                    ),
                    (
                        KILL_FILE.into(),
                        fs.create_node_and_allocate_node_id(
                            KillFile::new_node(cgroup.clone()),
                            FsNodeInfo::new(mode!(IFREG, 0o200), owner.clone()),
                        ),
                    ),
                    (
                        TYPE_FILE.into(),
                        fs.create_node_and_allocate_node_id(
                            StubBytesFile::new_node_with_data(
                                bug_ref!("https://fxbug.dev/489195565"),
                                b"domain".to_vec(),
                            ),
                            FsNodeInfo::new(mode!(IFREG, 0o644), owner.clone()),
                        ),
                    ),
                    (
                        SUBTREE_CONTROL_FILE.into(),
                        fs.create_node_and_allocate_node_id(
                            StubBytesFile::new_node_with_data(
                                bug_ref!("https://fxbug.dev/489194399"),
                                b"".to_vec(),
                            ),
                            FsNodeInfo::new(mode!(IFREG, 0o644), owner.clone()),
                        ),
                    ),
                ]);
            }
            CgroupVersion::V1(key) => {
                // TODO(https://fxbug.dev/401298305): "tasks" should contain task IDs (TIDs)
                // of all threads in the cgroup, and allow migrating individual threads. Currently
                // it uses `ControlGroupNode` which only supports process IDs (PIDs).
                interface_files.insert(
                    TASKS_FILE.into(),
                    fs.create_node_and_allocate_node_id(
                        ControlGroupNode::new(cgroup.clone()),
                        FsNodeInfo::new(mode!(IFREG, 0o644), owner.clone()),
                    ),
                );
                interface_files.insert(
                    PROCS_FILE.into(),
                    fs.create_node_and_allocate_node_id(
                        ControlGroupNode::new(cgroup.clone()),
                        FsNodeInfo::new(mode!(IFREG, 0o644), owner.clone()),
                    ),
                );
                if key.controllers.contains(&starnix_core::task::ControllerType::Cpuset) {
                    interface_files.insert(
                        CPUS_FILE.into(),
                        fs.create_node_and_allocate_node_id(
                            CpusetCpusFile::new_node(cgroup.clone()),
                            FsNodeInfo::new(mode!(IFREG, 0o644), owner.clone()),
                        ),
                    );
                }
            }
        }
        CgroupDirectoryHandle(Arc::new(Self {
            cgroup,
            interface_files: interface_files.into(),
            dir_nodes: Arc::downgrade(dir_nodes),
        }))
    }

    fn cgroup(&self) -> Result<Arc<dyn CgroupOps>, Errno> {
        self.cgroup.upgrade().ok_or_else(|| errno!(ENODEV))
    }

    fn dir_nodes(&self) -> Result<Arc<DirectoryNodes>, Errno> {
        self.dir_nodes.upgrade().ok_or_else(|| errno!(ENODEV))
    }

    fn version(&self) -> Result<CgroupVersion, Errno> {
        Ok(self.dir_nodes()?.version.clone())
    }

    #[cfg(test)]
    pub fn has_interface_files(&self) -> bool {
        !self.interface_files.lock().is_empty()
    }
}

/// `CgroupDirectoryHandle` is needed to implement a starnix_core trait for an `Arc<CgroupDirectory>`.
#[derive(Debug, Clone)]
pub struct CgroupDirectoryHandle(Arc<CgroupDirectory>);

impl Deref for CgroupDirectoryHandle {
    type Target = CgroupDirectory;

    fn deref(&self) -> &Self::Target {
        &self.0.deref()
    }
}

impl FsNodeOps for CgroupDirectoryHandle {
    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let children = self.cgroup()?.get_children()?;
        let nodes = self.dir_nodes()?.get_nodes(&children);
        assert_eq!(children.len(), nodes.len());

        let children_entries =
            children.into_iter().zip(nodes.into_iter()).filter_map(|(cgroup, maybe_node)| {
                // Ignore cgroups that hasn't been created or has been deleted from the hierarchy.
                let Some(node) = maybe_node else {
                    return None;
                };
                Some(VecDirectoryEntry {
                    entry_type: DirectoryEntryType::DIR,
                    name: cgroup.name().into(),
                    inode: Some(node.ino),
                })
            });

        let interface_files = self.interface_files.lock();
        let interface_entries = interface_files.iter().map(|(name, child)| VecDirectoryEntry {
            entry_type: DirectoryEntryType::REG,
            name: name.clone(),
            inode: Some(child.ino),
        });

        Ok(VecDirectory::new_file(children_entries.chain(interface_entries).collect()))
    }

    fn mkdir(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
        _mode: FileMode,
        owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        let dir_nodes = self.dir_nodes()?;
        let cgroup = self.cgroup()?.new_child(name)?;
        let directory = CgroupDirectory::new(
            Arc::downgrade(&cgroup) as Weak<dyn CgroupOps>,
            &node.fs(),
            &dir_nodes,
            owner.clone(),
        );
        let child = dir_nodes.add_node(&cgroup, directory, &node.fs(), owner);

        node.update_info(|info| {
            info.link_count += 1;
        });

        Ok(child)
    }

    fn mknod(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _mode: FileMode,
        _dev: DeviceId,
        _owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        error!(EACCES)
    }

    fn unlink(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
        child: &FsNodeHandle,
    ) -> Result<(), Errno> {
        // Only cgroup directories can be removed. Cgroup interface files cannot be removed.
        let Some(child_node) = child.downcast_ops::<CgroupDirectoryHandle>() else {
            return error!(EPERM);
        };

        let dir_nodes = self.dir_nodes()?;
        let child_cgroup = child_node.cgroup()?;
        let removed = self.cgroup()?.remove_child(name)?;
        assert!(Arc::ptr_eq(&(removed.clone() as Arc<dyn CgroupOps>), &child_cgroup));

        dir_nodes.remove_node(&removed)?;

        node.update_info(|info| {
            info.link_count -= 1;
        });

        Ok(())
    }

    fn create_symlink(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _target: &FsStr,
        _owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        error!(EPERM)
    }

    fn lookup(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        let interface_files = self.interface_files.lock();
        if let Some(node) = interface_files.get(name) {
            Ok(node.clone())
        } else {
            let cgroup = self.cgroup()?.get_child(name)?;
            self.dir_nodes()?.get_node(&cgroup)
        }
    }
}
