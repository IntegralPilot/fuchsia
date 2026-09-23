// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::PAGE_SIZE;
use crate::security;
use crate::task::{CurrentTask, Kernel};
use crate::vfs::memory_directory::MemoryDirectoryFile;
use crate::vfs::{
    CacheMode, DirEntry, DirEntryHandle, FileOps, FileSystem, FileSystemHandle, FileSystemOps,
    FileSystemOptions, FsNode, FsNodeHandle, FsNodeInfo, FsNodeOps, FsStr, FsString,
    MemoryRegularNode, MemoryXattrStorage, RenameContext, SymlinkNode, XattrStorage as _, fs_args,
    fs_node_impl_not_dir, fs_node_impl_xattr_delegate,
};
use starnix_logging::{log_warn, track_stub};
use starnix_types::vfs::default_statfs;
use starnix_uapi::auth::FsCred;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::{FileMode, mode};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::seal_flags::SealFlags;
use starnix_uapi::{TMPFS_MAGIC, error, gid_t, statfs, uid_t};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

pub struct TmpFs {
    name: &'static FsStr,
    casefold: bool,
}

impl FileSystemOps for Arc<TmpFs> {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(statfs {
            // Pretend we have a ton of free space.
            f_blocks: 0x100000000,
            f_bavail: 0x100000000,
            f_bfree: 0x100000000,
            ..default_statfs(TMPFS_MAGIC)
        })
    }
    fn name(&self) -> &'static FsStr {
        self.name
    }

    fn has_casefold_support(&self) -> bool {
        self.casefold
    }

    fn rename(
        &self,
        _fs: &FileSystem,
        _current_task: &CurrentTask,
        context: &mut RenameContext<'_>,
        _old_name: &FsStr,
        _new_name: &FsStr,
    ) -> Result<(), Errno> {
        fn child_count(node: &FsNodeHandle) -> &AtomicU32 {
            // The following cast are safe, unless something is seriously wrong:
            // - The filesystem should not be asked to rename node that it doesn't handle.
            // - Parents in a rename operation need to be directories.
            // - TmpFsDirectory is the ops for directories in this filesystem.
            &node.downcast_ops::<TmpFsDirectory>().unwrap().child_count
        }
        let replaced = context.replaced.map(|r| &r.node);
        let renamed_is_dir = context.renamed_is_dir();
        let replaced_is_dir = context.replaced_is_dir();

        if let Some(replaced) = replaced {
            if replaced_is_dir {
                // Ensures that replaces is empty.
                if child_count(replaced).load(Ordering::Acquire) != 0 {
                    return error!(ENOTEMPTY);
                }
            }
        }

        let has_replaced = replaced.is_some();

        // Update child counts without intermediate churn. We borrow parent nodes within a scoped
        // block before calling parent_infos_mut to avoid Arc::clone() overhead as well as borrow
        // checker conflicts with context methods.
        {
            let old_parent = &context.old_parent().node;
            let new_parent = &context.new_parent().node;
            let same_parent = Arc::ptr_eq(old_parent, new_parent);

            if same_parent {
                // Renaming within the same directory only changes the count if an entry was replaced.
                if has_replaced {
                    child_count(old_parent).fetch_sub(1, Ordering::Release);
                }
            } else {
                // Moving across directories removes one entry from old_parent, and only adds to
                // new_parent if we did not overwrite an existing entry.
                child_count(old_parent).fetch_sub(1, Ordering::Release);
                if !has_replaced {
                    child_count(new_parent).fetch_add(1, Ordering::Release);
                }
            }
        }

        // Update parent directory link counts for subdirectories (".." references).

        let (old_parent_info, mut new_parent_info) = context.parent_infos_mut();
        if let Some(new_info) = new_parent_info.as_deref_mut() {
            if renamed_is_dir {
                old_parent_info.link_count -= 1;
                new_info.link_count += 1;
            }
            if replaced_is_dir {
                new_info.link_count -= 1;
            }
        } else if replaced_is_dir {
            old_parent_info.link_count -= 1;
        }
        Ok(())
    }

    fn exchange(
        &self,
        _fs: &FileSystem,
        _current_task: &CurrentTask,
        context: &mut RenameContext<'_>,
        _name1: &FsStr,
        _name2: &FsStr,
    ) -> Result<(), Errno> {
        let is_dir1 = context.renamed_is_dir();
        let is_dir2 = context.replaced_is_dir();
        let (parent1_info, mut parent2_info) = context.parent_infos_mut();
        if let Some(parent2_info) = parent2_info.as_deref_mut() {
            if is_dir1 != is_dir2 {
                if is_dir1 {
                    parent1_info.link_count -= 1;
                    parent2_info.link_count += 1;
                } else {
                    parent1_info.link_count += 1;
                    parent2_info.link_count -= 1;
                }
            }
        }

        Ok(())
    }
}

pub fn tmp_fs(
    current_task: &CurrentTask,
    options: FileSystemOptions,
) -> Result<FileSystemHandle, Errno> {
    TmpFs::new_fs_with_options(&current_task.kernel(), options)
}

impl TmpFs {
    pub fn new_fs(kernel: &Kernel) -> FileSystemHandle {
        Self::new_fs_with_options(kernel, Default::default()).expect("empty options cannot fail")
    }

    pub fn new_fs_with_name(kernel: &Kernel, name: &'static FsStr) -> FileSystemHandle {
        Self::new_fs_with_options_and_name(kernel, Default::default(), name)
            .expect("empty options cannot fail")
    }

    pub fn new_fs_with_options(
        kernel: &Kernel,
        options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        Self::new_fs_with_options_and_name(kernel, options, "tmpfs".into())
    }

    fn new_fs_with_options_and_name(
        kernel: &Kernel,
        options: FileSystemOptions,
        name: &'static FsStr,
    ) -> Result<FileSystemHandle, Errno> {
        let mut mount_options = options.params.clone();
        let mode = if let Some(mode) = mount_options.remove(b"mode") {
            FileMode::from_string(mode.as_ref())?
        } else {
            mode!(IFDIR, 0o1777)
        };
        let uid = if let Some(uid) = mount_options.remove(b"uid") {
            fs_args::parse::<uid_t>(uid.as_ref())?
        } else {
            0
        };
        let gid = if let Some(gid) = mount_options.remove(b"gid") {
            fs_args::parse::<gid_t>(gid.as_ref())?
        } else {
            0
        };
        let casefold = mount_options.remove(b"casefold").is_some();
        let fs = FileSystem::new(
            kernel,
            CacheMode::Permanent,
            Arc::new(TmpFs { name, casefold }),
            options,
        )?;
        let root_ino = fs.allocate_ino();
        let mut info = FsNodeInfo::new(mode!(IFDIR, 0o1777), FsCred { uid, gid });
        info.chmod(mode);
        fs.create_root_with_info(root_ino, TmpFsDirectory::new(), info);

        if !mount_options.is_empty() {
            track_stub!(
                TODO("https://fxbug.dev/322873419"),
                "unknown tmpfs options, see logs for strings"
            );
            log_warn!("Unknown tmpfs options: {}", mount_options);
        }

        Ok(fs)
    }

    pub fn set_initial_content(kernel: &Kernel, fs: &FileSystemHandle, data: TmpFsData) {
        fn create_dir_entry_from_data(
            kernel: &Kernel,
            fs: &FileSystemHandle,
            data: TmpFsData,
            this: Option<DirEntryHandle>,
            name: FsString,
        ) -> DirEntryHandle {
            // TODO: https://fxbug.dev/455771186 - Revise FsNode initialization to better ensure
            // that all the things are appropriately labeled.
            let new_direntry = |node, parent, name| {
                let dir_entry = DirEntry::new(node, parent, name);
                security::fs_node_init_with_dentry_deferred(kernel, &dir_entry);
                dir_entry
            };

            match data.node_type {
                TmpFsNodeType::Link(target) => {
                    assert!(this.is_none());
                    let node = TmpFsDirectory::new_symlink(fs, target.as_ref(), data.owner);
                    new_direntry(node, None, name)
                }
                TmpFsNodeType::Directory(children) => {
                    let this = this.unwrap_or_else(|| {
                        let info = FsNodeInfo::new(mode!(IFDIR, data.perm), data.owner);
                        let node = fs.create_node_and_allocate_node_id(TmpFsDirectory::new(), info);
                        new_direntry(node, None, name.clone())
                    });
                    // Each child subdirectory contributes an extra link to the parent for its ".." entry.
                    let subdir_count = children
                        .values()
                        .filter(|child| matches!(child.node_type, TmpFsNodeType::Directory(_)))
                        .count();
                    if subdir_count > 0 {
                        this.node.update_info(|info| {
                            info.link_count += subdir_count;
                        });
                    }
                    this.node
                        .downcast_ops::<TmpFsDirectory>()
                        .expect("directory must be from tmpfs")
                        .child_count
                        .fetch_add(children.len() as u32, Ordering::Release);
                    let children = children
                        .into_iter()
                        .map(|(name, data)| {
                            let child =
                                create_dir_entry_from_data(kernel, fs, data, None, name.clone());
                            (name, child)
                        })
                        .collect::<BTreeMap<_, _>>();
                    this.set_children(children);
                    this
                }
            }
        }

        create_dir_entry_from_data(kernel, fs, data, Some(Arc::clone(fs.root())), "".into());
    }
}

pub enum TmpFsNodeType {
    Link(FsString),
    Directory(BTreeMap<FsString, TmpFsData>),
}

pub struct TmpFsData {
    pub owner: FsCred,
    pub perm: u32,
    pub node_type: TmpFsNodeType,
}

pub struct TmpFsDirectory {
    xattrs: MemoryXattrStorage,
    /// Live entry count, used for non-blocking directory emptiness checks.
    ///
    /// This atomic is only responsible for cross-thread visibility of the child count,
    /// with link/unlink of children in the directory being guarded via synchronization
    /// on the VFS `DirEntry`.
    child_count: AtomicU32,
}

impl TmpFsDirectory {
    pub fn new() -> Self {
        Self { xattrs: MemoryXattrStorage::default(), child_count: AtomicU32::new(0) }
    }

    fn new_symlink(fs: &Arc<FileSystem>, target: &FsStr, owner: FsCred) -> FsNodeHandle {
        let (link, info) = SymlinkNode::new(target, owner);
        fs.create_node_and_allocate_node_id(link, info)
    }
}

fn create_child_node(
    parent: &FsNode,
    mode: FileMode,
    dev: DeviceId,
    owner: FsCred,
) -> Result<FsNodeHandle, Errno> {
    let ops: Box<dyn FsNodeOps> = match mode.fmt() {
        FileMode::IFREG => Box::new(MemoryRegularNode::new()?),
        FileMode::IFIFO | FileMode::IFBLK | FileMode::IFCHR | FileMode::IFSOCK => {
            Box::new(TmpFsSpecialNode::new())
        }
        _ => return error!(EACCES),
    };
    let mut info = FsNodeInfo::new(mode, owner);
    info.rdev = dev;
    // blksize is PAGE_SIZE for in memory node.
    info.blksize = *PAGE_SIZE as usize;
    let child = parent.fs().create_node_and_allocate_node_id(ops, info);
    if mode.fmt() == FileMode::IFREG {
        // For files created in tmpfs, forbid sealing, by sealing the seal operation.
        child.write_guard_state.lock().enable_sealing(SealFlags::SEAL);
    }
    Ok(child)
}

impl FsNodeOps for TmpFsDirectory {
    fs_node_impl_xattr_delegate!(self, self.xattrs);

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(MemoryDirectoryFile::new()))
    }

    fn mkdir(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        mode: FileMode,
        owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        node.update_info(|info| {
            info.link_count += 1;
        });
        self.child_count.fetch_add(1, Ordering::Release);
        let mut info = FsNodeInfo::new(mode, owner);
        info.casefold = node.info().casefold;
        Ok(node.fs().create_node_and_allocate_node_id(TmpFsDirectory::new(), info))
    }

    fn mknod(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        mode: FileMode,
        dev: DeviceId,
        owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        let child = create_child_node(node, mode, dev, owner)?;
        self.child_count.fetch_add(1, Ordering::Release);
        Ok(child)
    }

    fn create_symlink(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        target: &FsStr,
        owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        self.child_count.fetch_add(1, Ordering::Release);
        Ok(Self::new_symlink(&node.fs(), target, owner))
    }

    fn create_tmpfile(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        mode: FileMode,
        owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        assert!(mode.is_reg());
        create_child_node(node, mode, DeviceId::NONE, owner)
    }

    fn link(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        child: &FsNodeHandle,
    ) -> Result<(), Errno> {
        child.update_info(|info| {
            info.link_count += 1;
        });
        self.child_count.fetch_add(1, Ordering::Release);
        Ok(())
    }

    fn unlink(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        child_to_unlink: &FsNodeHandle,
    ) -> Result<(), Errno> {
        if child_to_unlink.is_dir() {
            // The following cast is safe, unless something is seriously wrong:
            // - The filesystem should not be asked to unlink a node that it doesn't handle.
            // - The child has already been determined to be a directory.
            // - TmpFsDirectory is the ops for directories in this filesystem.
            let child_count =
                &child_to_unlink.downcast_ops::<TmpFsDirectory>().unwrap().child_count;
            if child_count.load(Ordering::Acquire) != 0 {
                return error!(ENOTEMPTY);
            }

            node.update_info(|info| {
                info.link_count -= 1;
            });
        }
        child_to_unlink.update_info(|info| {
            info.link_count -= 1;
        });
        self.child_count.fetch_sub(1, Ordering::Release);
        Ok(())
    }
}

struct TmpFsSpecialNode {
    xattrs: MemoryXattrStorage,
}

impl TmpFsSpecialNode {
    pub fn new() -> Self {
        Self { xattrs: MemoryXattrStorage::default() }
    }
}

impl FsNodeOps for TmpFsSpecialNode {
    fs_node_impl_not_dir!();
    fs_node_impl_xattr_delegate!(self, self.xattrs);

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        unreachable!("Special nodes cannot be opened.");
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing::spawn_kernel_and_run;
    use crate::vfs::buffers::{VecInputBuffer, VecOutputBuffer};
    use crate::vfs::fs_args::MountParams;
    use crate::vfs::{DirectoryMode, FdNumber, UnlinkKind};
    use starnix_uapi::errno;
    use starnix_uapi::mount_flags::FileSystemFlags;
    use starnix_uapi::vfs::ResolveFlags;
    use zerocopy::IntoBytes;

    #[::fuchsia::test]
    async fn test_tmpfs() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let fs = TmpFs::new_fs(&kernel);
            let root = fs.root();
            let usr = root.create_dir(&current_task, "usr".into()).unwrap();
            let _etc = root.create_dir(&current_task, "etc".into()).unwrap();
            let _usr_bin = usr.create_dir(&current_task, "bin".into()).unwrap();
            let mut names = root.copy_child_names();
            names.sort();
            assert!(names.iter().eq(["etc", "usr"].iter()));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_write_read() {
        spawn_kernel_and_run(async |current_task| {
            let path = "test.bin";
            let _file = current_task
                .fs()
                .root()
                .create_node(&current_task, path.into(), mode!(IFREG, 0o777), DeviceId::NONE)
                .unwrap();

            let wr_file = current_task.open_file(path.into(), OpenFlags::RDWR).unwrap();

            let test_seq = 0..10000u16;
            let test_vec = test_seq.collect::<Vec<_>>();
            let test_bytes = test_vec.as_slice().as_bytes();

            let written =
                wr_file.write(&current_task, &mut VecInputBuffer::new(test_bytes)).unwrap();
            assert_eq!(written, test_bytes.len());

            let mut read_buffer = VecOutputBuffer::new(test_bytes.len() + 1);
            let read = wr_file.read_at(&current_task, 0, &mut read_buffer).unwrap();
            assert_eq!(read, test_bytes.len());
            assert_eq!(test_bytes, read_buffer.data());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_read_past_eof() {
        spawn_kernel_and_run(async |current_task| {
            // Open an empty file
            let path = "test.bin";
            let _file = current_task
                .fs()
                .root()
                .create_node(&current_task, path.into(), mode!(IFREG, 0o777), DeviceId::NONE)
                .unwrap();
            let rd_file = current_task.open_file(path.into(), OpenFlags::RDONLY).unwrap();

            // Verify that attempting to read past the EOF (i.e. at a non-zero offset) returns 0
            let buffer_size = 0x10000;
            let mut output_buffer = VecOutputBuffer::new(buffer_size);
            let test_offset = 100;
            let result = rd_file.read_at(&current_task, test_offset, &mut output_buffer).unwrap();
            assert_eq!(result, 0);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_permissions() {
        spawn_kernel_and_run(async |current_task| {
            let path = "test.bin";
            let file = current_task
                .open_file_at(
                    FdNumber::AT_FDCWD,
                    path.into(),
                    OpenFlags::CREAT | OpenFlags::RDONLY,
                    FileMode::from_bits(0o777),
                    ResolveFlags::empty(),
                )
                .expect("failed to create file");
            assert_eq!(
                0,
                file.read(&current_task, &mut VecOutputBuffer::new(0)).expect("failed to read")
            );

            assert!(file.write(&current_task, &mut VecInputBuffer::new(&[])).is_err());

            let file = current_task
                .open_file_at(
                    FdNumber::AT_FDCWD,
                    path.into(),
                    OpenFlags::WRONLY,
                    FileMode::EMPTY,
                    ResolveFlags::empty(),
                )
                .expect("failed to open file WRONLY");

            assert!(file.read(&current_task, &mut VecOutputBuffer::new(0)).is_err());

            assert_eq!(
                0,
                file.write(&current_task, &mut VecInputBuffer::new(&[])).expect("failed to write")
            );

            let file = current_task
                .open_file_at(
                    FdNumber::AT_FDCWD,
                    path.into(),
                    OpenFlags::RDWR,
                    FileMode::EMPTY,
                    ResolveFlags::empty(),
                )
                .expect("failed to open file RDWR");

            assert_eq!(
                0,
                file.read(&current_task, &mut VecOutputBuffer::new(0)).expect("failed to read")
            );

            assert_eq!(
                0,
                file.write(&current_task, &mut VecInputBuffer::new(&[])).expect("failed to write")
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_persistence() {
        spawn_kernel_and_run(async |current_task| {
            {
                let root = &current_task.fs().root().entry;
                let usr =
                    root.create_dir(&current_task, "usr".into()).expect("failed to create usr");
                root.create_dir(&current_task, "etc".into()).expect("failed to create usr/etc");
                usr.create_dir(&current_task, "bin".into()).expect("failed to create usr/bin");
            }

            // At this point, all the nodes are dropped.

            current_task
                .open_file("/usr/bin".into(), OpenFlags::RDONLY | OpenFlags::DIRECTORY)
                .expect("failed to open /usr/bin");
            assert_eq!(
                errno!(ENOENT),
                current_task.open_file("/usr/bin/test.txt".into(), OpenFlags::RDWR).unwrap_err()
            );
            current_task
                .open_file_at(
                    FdNumber::AT_FDCWD,
                    "/usr/bin/test.txt".into(),
                    OpenFlags::RDWR | OpenFlags::CREAT,
                    FileMode::from_bits(0o777),
                    ResolveFlags::empty(),
                )
                .expect("failed to create test.txt");
            let txt = current_task
                .open_file("/usr/bin/test.txt".into(), OpenFlags::RDWR)
                .expect("failed to open test.txt");

            let usr_bin = current_task
                .open_file("/usr/bin".into(), OpenFlags::RDONLY)
                .expect("failed to open /usr/bin");
            usr_bin
                .name
                .unlink(
                    &current_task,
                    "test.txt".into(),
                    UnlinkKind::NonDirectory,
                    DirectoryMode::AllowAny,
                )
                .expect("failed to unlink test.text");
            assert_eq!(
                errno!(ENOENT),
                current_task.open_file("/usr/bin/test.txt".into(), OpenFlags::RDWR).unwrap_err()
            );
            assert_eq!(
                errno!(ENOENT),
                usr_bin
                    .name
                    .unlink(
                        &current_task,
                        "test.txt".into(),
                        UnlinkKind::NonDirectory,
                        DirectoryMode::AllowAny,
                    )
                    .unwrap_err()
            );

            assert_eq!(
                0,
                txt.read(&current_task, &mut VecOutputBuffer::new(0)).expect("failed to read")
            );
            std::mem::drop(txt);
            assert_eq!(
                errno!(ENOENT),
                current_task.open_file("/usr/bin/test.txt".into(), OpenFlags::RDWR).unwrap_err()
            );
            std::mem::drop(usr_bin);

            let usr = current_task
                .open_file("/usr".into(), OpenFlags::RDONLY)
                .expect("failed to open /usr");
            assert_eq!(
                errno!(ENOENT),
                current_task.open_file("/usr/foo".into(), OpenFlags::RDONLY).unwrap_err()
            );
            usr.name
                .unlink(&current_task, "bin".into(), UnlinkKind::Directory, DirectoryMode::AllowAny)
                .expect("failed to unlink /usr/bin");
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_data() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let fs = TmpFs::new_fs_with_options(
                &kernel,
                FileSystemOptions {
                    source: Default::default(),
                    flags: FileSystemFlags::empty().into(),
                    params: MountParams::parse(b"mode=0123,uid=42,gid=84".into())
                        .expect("parsed correctly"),
                },
            )
            .expect("new_fs");
            let info = fs.root().node.info();
            assert_eq!(info.mode, mode!(IFDIR, 0o123));
            assert_eq!(info.uid, 42);
            assert_eq!(info.gid, 84);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_set_initial_content_link_count() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let fs = TmpFs::new_fs(&kernel);
            let mut children = BTreeMap::new();
            children.insert(
                "dir1".into(),
                TmpFsData {
                    owner: FsCred::root(),
                    perm: 0o755,
                    node_type: TmpFsNodeType::Directory(BTreeMap::new()),
                },
            );
            children.insert(
                "link1".into(),
                TmpFsData {
                    owner: FsCred::root(),
                    perm: 0o755,
                    node_type: TmpFsNodeType::Link("target".into()),
                },
            );
            let initial = TmpFsData {
                owner: FsCred::root(),
                perm: 0o755,
                node_type: TmpFsNodeType::Directory(children),
            };
            TmpFs::set_initial_content(&kernel, &fs, initial);
            assert_eq!(fs.root().node.info().link_count, 3);
        })
        .await;
    }
}
