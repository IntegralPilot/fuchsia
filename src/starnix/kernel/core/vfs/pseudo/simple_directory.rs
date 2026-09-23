// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::CurrentTask;
use crate::vfs::{
    CloseFreeSafe, DirectoryEntryType, DirentSink, FileObject, FileOps, FileSystemHandle, FsNode,
    FsNodeHandle, FsNodeInfo, FsNodeOps, FsStr, FsString, SymlinkNode, emit_dotdot,
    fileops_impl_directory, fileops_impl_noop_sync, fileops_impl_unbounded_seek,
    fs_node_impl_dir_readonly,
};
use starnix_sync::{LockDepMutex, SimpleDirectoryEntriesLock};
use starnix_uapi::auth::FsCred;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::{FileMode, mode};
use starnix_uapi::open_flags::OpenFlags;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Helper used to populate a `SimpleDirectory` with nodes for a specific `FileSystem`.
pub struct SimpleDirectoryMutator {
    fs: FileSystemHandle,
    pub directory: Arc<SimpleDirectory>,
}

impl SimpleDirectoryMutator {
    /// Creates a mutator that will allocate nodes in `fs` and insert them into `directory`.
    pub fn new(fs: FileSystemHandle, directory: Arc<SimpleDirectory>) -> Self {
        Self { fs, directory }
    }

    pub fn node(&self, name: FsString, node: FsNodeHandle) {
        self.directory.entries.lock().insert(name, node);
    }

    pub fn entry(&self, name: &str, ops: impl Into<Box<dyn FsNodeOps>>, mode: FileMode) {
        let name: FsString = name.into();
        let node =
            self.fs.create_node_and_allocate_node_id(ops, FsNodeInfo::new(mode, FsCred::root()));
        self.node(name, node);
    }

    pub fn entry_etc(
        &self,
        name: FsString,
        ops: impl Into<Box<dyn FsNodeOps>>,
        mode: FileMode,
        dev: DeviceId,
        creds: FsCred,
    ) {
        let mut info = FsNodeInfo::new(mode, creds);
        info.rdev = dev;
        let node = self.fs.create_node_and_allocate_node_id(ops, info);
        self.node(name, node);
    }

    pub fn symlink(&self, name: &FsStr, target: &FsStr) {
        let (ops, info) = SymlinkNode::new(target, FsCred::root());
        let node = self.fs.create_node_and_allocate_node_id(ops, info);
        self.node(name.into(), node);
    }

    pub fn subdir(&self, name: &str, mode: u32, build_subdir: impl FnOnce(&Self)) {
        let name: &FsStr = name.into();
        self.subdir2(name, mode, build_subdir);
    }

    // TODO: Figure out a better way to overload this function for &str and &FsStr.
    pub fn subdir2(&self, name: &FsStr, mode: u32, build_subdir: impl FnOnce(&Self)) {
        let dir = self.directory.subdir(&self.fs, name, mode);
        let mutator = SimpleDirectoryMutator::new(self.fs.clone(), dir);
        build_subdir(&mutator);
    }

    pub fn remove(&self, name: &FsStr) {
        self.directory.remove(name);
    }
}

/// Common implementation of a simple read-only directory `FsNodeOps`.
///
/// `SimpleDirectoryMutator` is used to populate the directory with child `FsNode`s allocated
/// in the desired (usually kernel-internal, e.g. "sysfs", "proc", etc) filesystem.
pub struct SimpleDirectory {
    entries: LockDepMutex<BTreeMap<FsString, FsNodeHandle>, SimpleDirectoryEntriesLock>,
    not_found_handler:
        Box<dyn Fn(&FsStr, &BTreeMap<FsString, FsNodeHandle>) -> Errno + Send + Sync + 'static>,
}

impl SimpleDirectory {
    /// Returns a new instance with a default handler that returns `ENOENT` and logs context
    /// when a child is not found.
    pub fn new() -> Arc<Self> {
        Self::new_with_handler(|name, locked_entries| {
            errno!(
                ENOENT,
                format!(
                    "looking for {name} in {:?}",
                    locked_entries.keys().map(|e| e.to_string()).collect::<Vec<_>>()
                )
            )
        })
    }

    /// Returns a new instance configured to call the supplied `not_found_handler` whenever
    /// `FsNodeOps::lookup()` is called for an unknown child path.
    ///
    /// The handler is invoked with the `name` of the requested child and a reference to
    /// the current directory `entries`.
    pub fn new_with_handler(
        not_found_handler: impl Fn(&FsStr, &BTreeMap<FsString, FsNodeHandle>) -> Errno
        + Send
        + Sync
        + 'static,
    ) -> Arc<Self> {
        let not_found_handler = Box::new(not_found_handler);
        Arc::new(SimpleDirectory { entries: Default::default(), not_found_handler })
    }

    pub fn remove(&self, name: &FsStr) {
        self.entries.lock().remove(name);
    }

    fn walk<'a>(self: &Arc<Self>, path: &'a FsStr) -> Option<(Arc<Self>, &'a FsStr)> {
        fn check_component(component: &FsStr) {
            assert!(!component.is_empty());

            let dot: &FsStr = b".".into();
            assert_ne!(component, dot);

            let dotdot: &FsStr = b"..".into();
            assert_ne!(component, dotdot);
        }

        let mut components = path.split(|c| *c == b'/');
        let basename = components.next_back()?;
        let basename: &FsStr = basename.into();
        check_component(basename);
        let mut parent = self.clone();
        while let Some(component) = components.next() {
            let component: &FsStr = component.into();
            check_component(component);
            let Some(next) = parent.get_dir(component) else {
                return None;
            };
            parent = next;
        }
        Some((parent, basename))
    }

    pub fn edit(
        self: &Arc<Self>,
        fs: &FileSystemHandle,
        callback: impl FnOnce(&SimpleDirectoryMutator),
    ) {
        let mutator = SimpleDirectoryMutator::new(fs.clone(), self.clone());
        callback(&mutator);
    }

    pub fn subdir(&self, fs: &FileSystemHandle, name: &FsStr, mode: u32) -> Arc<SimpleDirectory> {
        let mut entries = self.entries.lock();
        if let Some(node) = entries.get(name) {
            assert!(node.info().mode == mode!(IFDIR, mode));
            let dir =
                node.downcast_ops::<Arc<SimpleDirectory>>().expect("subdir is a SimpleDirectory");
            dir.clone()
        } else {
            let dir = SimpleDirectory::new();
            let info = FsNodeInfo::new(mode!(IFDIR, mode), FsCred::root());
            let node = fs.create_node_and_allocate_node_id(dir.clone(), info);
            entries.insert(name.into(), node);
            dir
        }
    }

    fn get(&self, name: &FsStr) -> Option<FsNodeHandle> {
        let entries = self.entries.lock();
        entries.get(name).cloned()
    }

    fn get_dir(&self, name: &FsStr) -> Option<Arc<SimpleDirectory>> {
        let entries = self.entries.lock();
        entries
            .get(name)
            .and_then(|node| node.downcast_ops::<Arc<SimpleDirectory>>())
            .map(Arc::clone)
    }

    pub fn lookup(self: &Arc<Self>, path: &FsStr) -> Option<FsNodeHandle> {
        let (parent, basename) = self.walk(path)?;
        parent.get(basename)
    }

    pub fn into_node(self: Arc<Self>, fs: &FileSystemHandle, mode: u32) -> FsNodeHandle {
        let info = FsNodeInfo::new(mode!(IFDIR, mode), FsCred::root());
        fs.create_node_and_allocate_node_id(self, info)
    }
}

impl FsNodeOps for Arc<SimpleDirectory> {
    fs_node_impl_dir_readonly!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(self.clone()))
    }

    fn lookup(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        let entries = self.entries.lock();
        entries.get(name).cloned().ok_or_else(|| (self.not_found_handler)(name, &entries))
    }
}

/// `SimpleDirectory` doesn't implement the `close` method.
impl CloseFreeSafe for SimpleDirectory {}
impl FileOps for SimpleDirectory {
    fileops_impl_directory!();
    fileops_impl_noop_sync!();
    fileops_impl_unbounded_seek!();

    fn readdir(
        &self,
        file: &FileObject,
        _current_task: &CurrentTask,
        sink: &mut dyn DirentSink,
    ) -> Result<(), Errno> {
        emit_dotdot(file, sink)?;

        // Skip through the entries until the current offset is reached.
        // Subtract 2 from the offset to account for `.` and `..`.
        let entries = self.entries.lock();
        for (name, node) in entries.iter().skip(sink.offset() as usize - 2) {
            sink.add(
                node.ino,
                sink.offset() + 1,
                DirectoryEntryType::from_mode(node.info().mode),
                name.as_ref(),
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::spawn_kernel_and_run;
    use crate::vfs::FsNodeOps;
    use starnix_uapi::errno;

    #[fuchsia::test]
    async fn test_default_not_found_handler() {
        spawn_kernel_and_run(async |current_task| {
            let dir = SimpleDirectory::new();
            let node = dir.clone().into_node(&current_task.fs().root().entry.node.fs(), 0o777);
            let result = FsNodeOps::lookup(&dir, &node, &current_task, "nonexistent".into());
            assert_eq!(result.unwrap_err(), errno!(ENOENT));
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_custom_not_found_handler() {
        spawn_kernel_and_run(async |current_task| {
            let dir = SimpleDirectory::new_with_handler(|name, _entries| {
                if name == "special" { errno!(EACCES) } else { errno!(ENOENT) }
            });
            let node = dir.clone().into_node(&current_task.fs().root().entry.node.fs(), 0o777);

            let result_special = FsNodeOps::lookup(&dir, &node, &current_task, "special".into());
            assert_eq!(result_special.unwrap_err(), errno!(EACCES));

            let result_other = FsNodeOps::lookup(&dir, &node, &current_task, "other".into());
            assert_eq!(result_other.unwrap_err(), errno!(ENOENT));
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_simple_directory_lookups() {
        spawn_kernel_and_run(async |current_task| {
            let fs = current_task.fs().root().entry.node.fs();
            let dir = SimpleDirectory::new();
            let mutator = SimpleDirectoryMutator::new(fs.clone(), dir.clone());

            // Add a symlink
            mutator.symlink("link".into(), "target".into());

            // Add a subdir
            mutator.subdir("subdir", 0o755, |sub_mutator| {
                sub_mutator.symlink("sublink".into(), "subtarget".into());
            });

            let node = dir.clone().into_node(&fs, 0o777);

            // Verify that lookup returns the same FsNodeHandle for multiple calls.
            let node1 =
                FsNodeOps::lookup(&dir, &node, &current_task, "link".into()).expect("lookup link");
            let node2 = FsNodeOps::lookup(&dir, &node, &current_task, "link".into())
                .expect("lookup link again");

            assert!(Arc::ptr_eq(&node1, &node2));
            assert!(node1.info().mode.is_lnk());

            // Verify that lookup returns the same FsNodeHandle for subdirectories.
            let subdir1 = FsNodeOps::lookup(&dir, &node, &current_task, "subdir".into())
                .expect("lookup subdir");
            let subdir2 = FsNodeOps::lookup(&dir, &node, &current_task, "subdir".into())
                .expect("lookup subdir again");

            assert!(Arc::ptr_eq(&subdir1, &subdir2));
            assert!(subdir1.info().mode.is_dir());

            // Verify that the SimpleDirectory::lookup helper works for nested paths.
            let sublink = dir.lookup("subdir/sublink".into()).expect("lookup subdir/sublink");
            assert!(sublink.info().mode.is_lnk());

            // Verify that removing an entry works.
            mutator.remove("link".into());
            let result = FsNodeOps::lookup(&dir, &node, &current_task, "link".into());
            assert_eq!(result.unwrap_err(), errno!(ENOENT));
        })
        .await;
    }
}
