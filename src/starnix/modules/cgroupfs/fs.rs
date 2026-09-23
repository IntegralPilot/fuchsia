// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::security;
use starnix_core::task::{Cgroup, CgroupOps, CgroupRoot, CgroupV1Key, ControllerType, CurrentTask};
use starnix_core::vfs::{
    CacheMode, FileSystem, FileSystemHandle, FileSystemOps, FileSystemOptions, FsNodeHandle,
    FsNodeInfo, FsStr,
};
use starnix_logging::log_warn;
use starnix_sync::{CgroupDirectoryNodesLock, LockDepMutex};
use starnix_types::vfs::default_statfs;
use starnix_uapi::auth::FsCred;
use starnix_uapi::errors::Errno;
use starnix_uapi::{CGROUP_SUPER_MAGIC, CGROUP2_SUPER_MAGIC, errno, error, mode, statfs};

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Weak};

use crate::directory::{CgroupDirectory, CgroupDirectoryHandle};

pub struct CgroupV1Fs {
    pub root: Arc<CgroupRoot>,

    /// All directory nodes of the filesystem.
    pub dir_nodes: Arc<DirectoryNodes>,

    /// The name of this filesystem, which is also the name of the cgroup v1 hierarchy.
    /// E.g., "cgroup" or "cpuset".
    pub name: &'static FsStr,

    /// The key identifying this hierarchy in the global cgroup v1 state.
    pub hierarchy_key: CgroupV1Key,
}

impl CgroupV1Fs {
    pub fn new_fs(
        current_task: &CurrentTask,
        options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        Self::new_fs_inner(current_task, options, b"cgroup".into())
    }

    pub fn new_fs_cpuset(
        current_task: &CurrentTask,
        options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        Self::new_fs_inner(current_task, options, b"cpuset".into())
    }

    fn new_fs_inner(
        current_task: &CurrentTask,
        options: FileSystemOptions,
        fs_name: &'static FsStr,
    ) -> Result<FileSystemHandle, Errno> {
        let kernel = current_task.kernel();

        let mut params = options.params.clone();
        // Eat LSM/SELinux options (like `context=`) before running validation. Android's
        // `libprocessgroup` mounts cgroups with these options. If they are not stripped
        // by an active LSM, they would be treated as invalid controllers and fail the
        // strict validation below.
        let _ = security::sb_eat_lsm_opts(kernel, &mut params)?;

        let name = params.get(b"name").map(|n| String::from_utf8_lossy(n.as_ref()).to_string());

        let mut controllers = BTreeSet::new();
        for key in params.keys() {
            let key_str = String::from_utf8_lossy(key.as_ref());
            if let Ok(controller) = key_str.parse::<ControllerType>() {
                controllers.insert(controller);
            } else {
                // Ignore common cgroup options that we don't support yet to avoid spamming warnings.
                // TODO(https://fxbug.dev/322255433): Support these options.
                if key_str != "name"
                    && key_str != "none"
                    && key_str != "noprefix"
                    && key_str != "cpuset_v2_mode"
                    && key_str != "clone_children"
                {
                    log_warn!("cgroup v1: invalid controller or option: {}", key_str);
                }
                continue;
            }
        }

        if params.get(b"none").is_some() && !controllers.is_empty() {
            return error!(EINVAL);
        }

        if fs_name == b"cpuset" {
            // cpuset filesystem only allows cpuset controller.
            if !controllers.is_empty()
                && (controllers.len() > 1 || !controllers.contains(&ControllerType::Cpuset))
            {
                return error!(EINVAL);
            }
            controllers.insert(ControllerType::Cpuset);
        }

        if controllers.is_empty() && name.is_none() {
            log_warn!("Mounting cgroup v1 without controllers or name is not supported");
            return error!(EINVAL);
        }

        let root = kernel.cgroups.get_or_create_cgroup1(&controllers, name.as_deref())?;

        let hierarchy_key = CgroupV1Key { controllers, name };

        let dir_nodes =
            DirectoryNodes::new(Arc::downgrade(&root), CgroupVersion::V1(hierarchy_key.clone()));
        let root_dir = dir_nodes.root.clone();
        let fs = FileSystem::new(
            kernel,
            CacheMode::Uncached,
            CgroupV1Fs {
                dir_nodes: dir_nodes.clone(),
                root: root.clone(),
                name: fs_name,
                hierarchy_key,
            },
            options,
        )?;
        root_dir.create_root_interface_files(&fs);
        let root_ino = fs.allocate_ino();
        fs.create_root(root_ino, root_dir);

        // Populate existing child cgroups if any (e.g. on remount).
        dir_nodes.populate_from_root(&fs, &root, FsCred::root())?;

        Ok(fs)
    }
}
impl FileSystemOps for CgroupV1Fs {
    fn name(&self) -> &'static FsStr {
        self.name
    }
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(CGROUP_SUPER_MAGIC))
    }
}

pub struct CgroupV2Fs {
    /// All directory nodes of the filesystem.
    pub dir_nodes: Arc<DirectoryNodes>,
}

struct CgroupV2FsHandle(FileSystemHandle);
pub fn cgroup2_fs(
    current_task: &CurrentTask,
    options: FileSystemOptions,
) -> Result<FileSystemHandle, Errno> {
    Ok(current_task
        .kernel()
        .expando
        .get_or_try_init(|| Ok(CgroupV2FsHandle(CgroupV2Fs::new_fs(current_task, options)?)))?
        .0
        .clone())
}

impl CgroupV2Fs {
    fn new_fs(
        current_task: &CurrentTask,
        options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        let kernel = current_task.kernel();
        let dir_nodes =
            DirectoryNodes::new(Arc::downgrade(&kernel.cgroups.cgroup2), CgroupVersion::V2);
        let root = dir_nodes.root.clone();
        let fs = FileSystem::new(kernel, CacheMode::Uncached, CgroupV2Fs { dir_nodes }, options)?;
        root.create_root_interface_files(&fs);
        let root_ino = fs.allocate_ino();
        fs.create_root(root_ino, root);
        Ok(fs)
    }
}

impl FileSystemOps for CgroupV2Fs {
    fn name(&self) -> &'static FsStr {
        b"cgroup2".into()
    }
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(CGROUP2_SUPER_MAGIC))
    }
}

/// Represents all directory nodes of a cgroup hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CgroupVersion {
    V1(CgroupV1Key),
    V2,
}

pub struct DirectoryNodes {
    /// `CgroupRoot`'s directory handle. The `FileSystem` owns the `FsNode` of the root, and so we
    /// do not have a `FsNodeHandle` of the root.
    root: CgroupDirectoryHandle,

    /// All non-root cgroup directories, keyed by cgroup's ID. Every non-root cgroup has a
    /// corresponding node.
    nodes: LockDepMutex<HashMap<u64, FsNodeHandle>, CgroupDirectoryNodesLock>,

    /// The version of cgroup for this hierarchy (v1 or v2).
    pub version: CgroupVersion,
}

impl DirectoryNodes {
    pub fn new(root_cgroup: Weak<CgroupRoot>, version: CgroupVersion) -> Arc<DirectoryNodes> {
        Arc::new_cyclic(|weak_self| Self {
            root: CgroupDirectory::new_root(root_cgroup, weak_self.clone()),
            nodes: Default::default(),
            version,
        })
    }

    /// Looks for the corresponding node in the filesystem, errors if not found.
    pub fn get_node(&self, cgroup: &Arc<Cgroup>) -> Result<FsNodeHandle, Errno> {
        let nodes = self.nodes.lock();
        nodes.get(&cgroup.id()).cloned().ok_or_else(|| errno!(ENOENT))
    }

    /// Returns the corresponding nodes for a set of cgroups.
    pub fn get_nodes(&self, cgroups: &Vec<Arc<Cgroup>>) -> Vec<Option<FsNodeHandle>> {
        let nodes = self.nodes.lock();
        cgroups.iter().map(|cgroup| nodes.get(&cgroup.id()).cloned()).collect()
    }

    /// Creates a new `FsNode` for `directory` and stores it in `nodes`.
    pub fn add_node(
        &self,
        cgroup: &Arc<Cgroup>,
        directory: CgroupDirectoryHandle,
        fs: &FileSystemHandle,
        owner: FsCred,
    ) -> FsNodeHandle {
        let id = cgroup.id();
        let node = fs.create_node_and_allocate_node_id(
            directory,
            FsNodeInfo::new(mode!(IFDIR, 0o755), owner),
        );
        let mut nodes = self.nodes.lock();
        nodes.insert(id, node.clone());
        node
    }

    /// Removes an entry from `nodes`, errors if not found.
    pub fn remove_node(&self, cgroup: &Arc<Cgroup>) -> Result<FsNodeHandle, Errno> {
        let id = cgroup.id();
        let mut nodes = self.nodes.lock();
        nodes.remove(&id).ok_or_else(|| errno!(ENOENT))
    }

    pub fn populate_from_root(
        self: &Arc<Self>,
        fs: &FileSystemHandle,
        root: &Arc<CgroupRoot>,
        owner: FsCred,
    ) -> Result<(), Errno> {
        let children = root.get_children()?;
        for child in children {
            self.populate_recursive(fs, &child, owner.clone())?;
        }
        Ok(())
    }

    fn populate_recursive(
        self: &Arc<Self>,
        fs: &FileSystemHandle,
        cgroup: &Arc<Cgroup>,
        owner: FsCred,
    ) -> Result<(), Errno> {
        let directory = CgroupDirectory::new(
            Arc::downgrade(cgroup) as Weak<dyn CgroupOps>,
            fs,
            self,
            owner.clone(),
        );
        self.add_node(cgroup, directory, fs, owner.clone());

        let children = cgroup.get_children()?;
        for child in children {
            self.populate_recursive(fs, &child, owner.clone())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use starnix_core::testing::spawn_kernel_and_run;
    use starnix_core::vfs::FsNodeOps;
    use starnix_core::vfs::fs_args::MountParams;
    use starnix_core::vfs::fs_registry::FsRegistry;
    use starnix_uapi::file_mode::FileMode;
    use starnix_uapi::fs_type::FileSystemTypeFlags;

    #[::fuchsia::test]
    async fn test_filesystem_creates_nodes() {
        spawn_kernel_and_run(async move |current_task| {
            let kernel = current_task.kernel();
            let registry = kernel.expando.get::<FsRegistry>();
            registry.register(b"cgroup2".into(), FileSystemTypeFlags::empty(), cgroup2_fs);

            let fs = current_task
                .create_filesystem(b"cgroup2".into(), Default::default())
                .expect("create_filesystem");

            let cgroupfs = fs.downcast_ops::<CgroupV2Fs>().expect("downcast_ops");
            let dir_nodes = cgroupfs.dir_nodes.clone();
            assert!(dir_nodes.nodes.lock().is_empty(), "new filesystem does not contain nodes");

            let root_dir = dir_nodes.root.clone();
            assert!(root_dir.has_interface_files(), "root directory is initialized");
        })
        .await;
    }
    #[::fuchsia::test]
    async fn test_cgroup_v1_remount_preserves_tree() {
        spawn_kernel_and_run(async move |current_task| {
            let kernel = current_task.kernel();
            let registry = kernel.expando.get::<FsRegistry>();
            registry.register(b"cgroup".into(), FileSystemTypeFlags::empty(), CgroupV1Fs::new_fs);

            let options = FileSystemOptions {
                params: MountParams::parse(b"memory".into()).unwrap(),
                ..Default::default()
            };

            {
                let fs1 = current_task
                    .create_filesystem(b"cgroup".into(), options.clone())
                    .expect("create_filesystem");
                let cgroupfs1 = fs1.downcast_ops::<CgroupV1Fs>().expect("downcast_ops");
                let dir_nodes1 = cgroupfs1.dir_nodes.clone();
                let root_dir1 = dir_nodes1.root.clone();
                let root_node1 = fs1.root();

                root_dir1
                    .mkdir(
                        &root_node1.node,
                        current_task,
                        "test_child".into(),
                        FileMode::default(),
                        FsCred::root(),
                    )
                    .expect("mkdir");

                let lookup_result1 =
                    root_dir1.lookup(&root_node1.node, current_task, "test_child".into());
                assert!(lookup_result1.is_ok());
            }

            let fs2 = current_task
                .create_filesystem(b"cgroup".into(), options)
                .expect("create_filesystem");
            let cgroupfs2 = fs2.downcast_ops::<CgroupV1Fs>().expect("downcast_ops");
            let dir_nodes2 = cgroupfs2.dir_nodes.clone();
            let root_dir2 = dir_nodes2.root.clone();
            let root_node2 = fs2.root();

            let lookup_result2 =
                root_dir2.lookup(&root_node2.node, current_task, "test_child".into());
            assert!(lookup_result2.is_ok(), "child cgroup should be preserved on remount");
        })
        .await;
    }
}
