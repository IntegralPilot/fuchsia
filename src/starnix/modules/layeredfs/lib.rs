// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "512"]

use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::{
    CacheMode, DirectoryEntryType, DirentSink, FileHandle, FileObject, FileOps, FileSystem,
    FileSystemHandle, FileSystemOps, FsNode, FsNodeHandle, FsNodeOps, FsStr, FsString, MountInfo,
    SeekTarget, ValueOrSize, WhatToMount, XattrOp, fileops_impl_directory, fileops_impl_noop_sync,
    fs_node_impl_dir_readonly, unbounded_seek,
};

use starnix_uapi::errors::Errno;
use starnix_uapi::mount_flags::{FileSystemFlags, MountpointFlags};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{errno, ino_t, off_t, statfs};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

struct LayeredMountAction {
    path: FsString,
    fs: FileSystemHandle,
}

/// A callback used to complete the initialization of a `LayeredFs`.
///
/// After the `FileSystem` has been created by [`LayeredFsBuilder::build`], this closure
/// must be invoked to create the sub-mounts that layer the additional filesystems
/// at their specified paths.
pub type LayeredFsMounts = Box<dyn FnOnce(&CurrentTask) -> Result<(), Errno>>;

/// `FileSystem` builder that allows a set of auxiliary `FileSystem`s to be mounted at specified
/// paths relative to the base filesystem, regardless of whether the base filesystem has directories
/// at those paths, that may be mounted-onto.
///
/// Auxiliary `FileSystem`s and their mount paths are provided via calls to `add()`, and the layered
/// filesystem created using `build()`.
pub struct LayeredFsBuilder {
    fs: FileSystemHandle,
    subdirs: BTreeMap<FsString, LayeredFsBuilder>,
}

fn split_path(path: &FsStr) -> Vec<&FsStr> {
    path.split(|c| *c == b'/').map(<&FsStr>::from).collect()
}

impl LayeredFsBuilder {
    /// Returns a `LayeredFsBuilder` with `root_fs` as the underlying base filesystem.
    pub fn new(root_fs: FileSystemHandle) -> Self {
        Self { fs: root_fs, subdirs: Default::default() }
    }

    /// Specifies that filesystem `fs` should be mounted at the specified `path` relative to the
    /// base filesystem.
    ///
    /// `path` must specify an absolute path under the base filesystem (i.e. starting with "/").
    /// If `path` has multiple components then intermediate components must already have been
    /// added to the builder.
    pub fn add(&mut self, path: &str, fs: FileSystemHandle) {
        let path = FsStr::new(path);
        assert_eq!(path[0], b'/');
        let parts = split_path(&path[1..]);
        assert!(!parts.is_empty());
        let final_part = parts.len() - 1;

        let mut parent = self;
        for i in 0..final_part {
            parent = parent.subdirs.get_mut(parts[i]).unwrap();
        }

        parent.subdirs.insert(parts[parts.len() - 1].into(), Self::new(fs));
    }

    /// Returns the new `FileSystem` handle, and a finalization callback that must be invoked to
    /// set up the subordinate mount points.
    ///
    /// The underlying base `FileSystem` will be returned directly if no sub-mounts were specified
    /// via `add()`. Otherwise a `LayeredFs` instance will be returned, to provide stub directory
    /// entries for the sub-mounts to be mounted onto.
    pub fn build(self, kernel: &Kernel) -> (FileSystemHandle, LayeredFsMounts) {
        let (fs, actions) = self.build_internal(kernel, Default::default());
        let cb = Box::new(move |current_task: &CurrentTask| {
            for action in actions {
                let mount_point =
                    current_task.lookup_path_from_root(action.path.as_ref()).map_err(|e| {
                        Errno::with_context(
                            e.code,
                            format!("lookup path from root: {}", action.path),
                        )
                    })?;
                mount_point.mount(WhatToMount::Fs(action.fs), MountpointFlags::empty()).map_err(
                    |e| {
                        Errno::with_context(e.code, format!("mount layered fs at: {}", action.path))
                    },
                )?;
            }
            Ok(())
        });
        (fs, cb)
    }

    fn build_internal(
        self,
        kernel: &Kernel,
        prefix: &FsStr,
    ) -> (FileSystemHandle, Vec<LayeredMountAction>) {
        if self.subdirs.is_empty() {
            return (self.fs, Vec::new());
        }

        let names =
            self.subdirs.iter().map(|(name, entry)| (name.clone(), entry.fs.clone())).collect();
        let fs = LayeredFs::new_fs(kernel, self.fs, names);

        let mut mount_actions = Vec::new();
        for (subpath, builder) in self.subdirs {
            let path = FsString::from(format!("{}/{}", prefix, subpath));
            let (fs, subdir_actions) = builder.build_internal(kernel, path.as_ref());
            mount_actions.push(LayeredMountAction { path, fs });
            mount_actions.extend(subdir_actions.into_iter());
        }

        (fs, mount_actions)
    }
}

/// A filesystem that will delegate most operation to a base one, but have a number of top level
/// directory that points to other filesystems.
struct LayeredFs {
    base_fs: FileSystemHandle,
    mappings: BTreeMap<FsString, FileSystemHandle>,
}

impl LayeredFs {
    /// Build a new filesystem.
    ///
    /// `base_fs`: The base file system that this file system will delegate to.
    /// `mappings`: The map of top level directory to filesystems that will be layered on top of
    /// `base_fs`.
    fn new_fs(
        kernel: &Kernel,
        base_fs: FileSystemHandle,
        mappings: BTreeMap<FsString, FileSystemHandle>,
    ) -> FileSystemHandle {
        let options = base_fs.options.clone();
        let layered_fs = Arc::new(LayeredFs { base_fs, mappings });
        let fs = FileSystem::new(
            kernel,
            CacheMode::Uncached,
            LayeredFileSystemOps { fs: layered_fs.clone() },
            options,
        )
        .expect("layeredfs constructed with valid options");
        let root_ino = fs.allocate_ino();
        fs.create_root(root_ino, LayeredNodeOps { fs: layered_fs });
        fs
    }
}

struct LayeredFileSystemOps {
    fs: Arc<LayeredFs>,
}

impl FileSystemOps for LayeredFileSystemOps {
    fn statfs(&self, _fs: &FileSystem, current_task: &CurrentTask) -> Result<statfs, Errno> {
        self.fs.base_fs.statfs(current_task)
    }
    fn name(&self) -> &'static FsStr {
        self.fs.base_fs.name()
    }
    fn update_flags(
        &self,
        fs: &FileSystem,
        current_task: &CurrentTask,
        new_flags: FileSystemFlags,
    ) -> Result<(), Errno> {
        self.fs.base_fs.update_flags(current_task, new_flags)?;
        let flags = self.fs.base_fs.options.flags.load(Ordering::Relaxed);
        fs.options.flags.store(flags, Ordering::Relaxed);
        Ok(())
    }
}

struct LayeredNodeOps {
    fs: Arc<LayeredFs>,
}

impl FsNodeOps for LayeredNodeOps {
    fs_node_impl_dir_readonly!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        current_task: &CurrentTask,
        flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(LayeredFileOps {
            fs: self.fs.clone(),
            root_file: self.fs.base_fs.root().open_anonymous(current_task, flags)?,
        }))
    }

    fn lookup(
        &self,
        _node: &FsNode,
        current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        if let Some(fs) = self.fs.mappings.get(name) {
            Ok(fs.root().node.clone())
        } else {
            self.fs.base_fs.root().node.lookup(current_task, &MountInfo::detached(), name)
        }
    }

    fn get_xattr(
        &self,
        _node: &FsNode,
        current_task: &CurrentTask,
        name: &FsStr,
        max_size: usize,
    ) -> Result<ValueOrSize<FsString>, Errno> {
        self.fs.base_fs.root().node.ops().get_xattr(
            &*self.fs.base_fs.root().node,
            current_task,
            name,
            max_size,
        )
    }

    /// Set an extended attribute on the node.
    fn set_xattr(
        &self,
        _node: &FsNode,
        current_task: &CurrentTask,
        name: &FsStr,
        value: &FsStr,
        op: XattrOp,
    ) -> Result<(), Errno> {
        self.fs.base_fs.root().node.set_xattr(current_task, &MountInfo::detached(), name, value, op)
    }

    fn remove_xattr(
        &self,
        _node: &FsNode,
        current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<(), Errno> {
        self.fs.base_fs.root().node.remove_xattr(current_task, &MountInfo::detached(), name)
    }

    fn list_xattrs(
        &self,
        _node: &FsNode,
        current_task: &CurrentTask,
        max_size: usize,
    ) -> Result<ValueOrSize<Vec<FsString>>, Errno> {
        self.fs.base_fs.root().node.list_xattrs(current_task, max_size)
    }
}

struct LayeredFileOps {
    fs: Arc<LayeredFs>,
    root_file: FileHandle,
}

impl FileOps for LayeredFileOps {
    fileops_impl_directory!();
    fileops_impl_noop_sync!();

    fn seek(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        current_offset: off_t,
        target: SeekTarget,
    ) -> Result<off_t, Errno> {
        let mut new_offset = unbounded_seek(current_offset, target)?;
        if new_offset >= self.fs.mappings.len() as off_t {
            new_offset = self
                .root_file
                .seek(current_task, SeekTarget::Set(new_offset - self.fs.mappings.len() as off_t))?
                .checked_add(self.fs.mappings.len() as off_t)
                .ok_or_else(|| errno!(EINVAL))?;
        }
        Ok(new_offset)
    }

    fn readdir(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        sink: &mut dyn DirentSink,
    ) -> Result<(), Errno> {
        for (key, fs) in self.fs.mappings.iter().skip(sink.offset() as usize) {
            sink.add(fs.root().node.ino, sink.offset() + 1, DirectoryEntryType::DIR, key.as_ref())?;
        }

        struct DirentSinkWrapper<'a> {
            sink: &'a mut dyn DirentSink,
            mappings: &'a BTreeMap<FsString, FileSystemHandle>,
            offset: &'a mut off_t,
        }

        impl<'a> DirentSink for DirentSinkWrapper<'a> {
            fn add(
                &mut self,
                inode_num: ino_t,
                offset: off_t,
                entry_type: DirectoryEntryType,
                name: &FsStr,
            ) -> Result<(), Errno> {
                if !self.mappings.contains_key(name) {
                    self.sink.add(
                        inode_num,
                        offset + (self.mappings.len() as off_t),
                        entry_type,
                        name,
                    )?;
                }
                *self.offset = offset;
                Ok(())
            }
            fn offset(&self) -> off_t {
                *self.offset
            }
        }

        // Allow subclassing for FileObjectOffset because the lock on the
        // inner file's offset is acquired while holding the lock on the
        // outer (layered) file's offset.
        // This is safe because the locks are on different file instances
        // and follow a strict outer-to-inner hierarchy, preventing cycles.
        let _token = starnix_sync::allow_subclass();
        let mut root_file_offset = self.root_file.offset.copy();
        let mut wrapper =
            DirentSinkWrapper { sink, mappings: &self.fs.mappings, offset: &mut *root_file_offset };

        self.root_file.readdir(current_task, &mut wrapper)?;
        root_file_offset.update();
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use starnix_core::fs::tmpfs::TmpFs;
    use starnix_core::testing::*;

    fn get_root_entry_names(current_task: &CurrentTask, fs: &FileSystem) -> Vec<Vec<u8>> {
        struct DirentNameCapturer {
            pub names: Vec<Vec<u8>>,
            offset: off_t,
        }
        impl DirentSink for DirentNameCapturer {
            fn add(
                &mut self,
                _inode_num: ino_t,
                offset: off_t,
                _entry_type: DirectoryEntryType,
                name: &FsStr,
            ) -> Result<(), Errno> {
                self.names.push(name.to_vec());
                self.offset = offset;
                Ok(())
            }
            fn offset(&self) -> off_t {
                self.offset
            }
        }
        let mut sink = DirentNameCapturer { names: vec![], offset: 0 };
        fs.root()
            .open_anonymous(current_task, OpenFlags::RDONLY)
            .expect("open")
            .readdir(current_task, &mut sink)
            .expect("readdir");
        std::mem::take(&mut sink.names)
    }

    #[::fuchsia::test]
    async fn test_remove_duplicates() {
        spawn_kernel_and_run(async move |current_task| {
            let kernel = current_task.kernel();
            let base = TmpFs::new_fs(kernel);
            base.root().create_dir_for_testing(current_task, "d1".into()).expect("create_dir");
            base.root().create_dir_for_testing(current_task, "d2".into()).expect("create_dir");
            let base_entries = get_root_entry_names(current_task, &base);
            assert_eq!(base_entries.len(), 4);
            assert!(base_entries.contains(&b".".to_vec()));
            assert!(base_entries.contains(&b"..".to_vec()));
            assert!(base_entries.contains(&b"d1".to_vec()));
            assert!(base_entries.contains(&b"d2".to_vec()));

            let tmpfs1 = TmpFs::new_fs(kernel);
            let tmpfs2 = TmpFs::new_fs(kernel);
            let layered_fs = LayeredFs::new_fs(
                kernel,
                base,
                BTreeMap::from([("d1".into(), tmpfs1), ("d3".into(), tmpfs2)]),
            );
            let layered_fs_entries = get_root_entry_names(current_task, &layered_fs);
            assert_eq!(layered_fs_entries.len(), 5);
            assert!(layered_fs_entries.contains(&b".".to_vec()));
            assert!(layered_fs_entries.contains(&b"..".to_vec()));
            assert!(layered_fs_entries.contains(&b"d1".to_vec()));
            assert!(layered_fs_entries.contains(&b"d2".to_vec()));
            assert!(layered_fs_entries.contains(&b"d3".to_vec()));
        })
        .await;
    }
}
