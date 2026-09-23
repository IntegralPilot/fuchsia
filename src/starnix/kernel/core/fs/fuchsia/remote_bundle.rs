// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fs::fuchsia::update_info_from_attrs;
use crate::mm::memory::MemoryObject;
use crate::mm::{ProtectionFlags, VMEX_RESOURCE};
use crate::task::{CurrentTask, EventHandler, Kernel, WaitCanceler, Waiter};
use crate::vfs::{
    CacheConfig, CacheMode, DEFAULT_BYTES_PER_BLOCK, DirectoryEntryType, DirentSink, FileObject,
    FileOps, FileSystem, FileSystemHandle, FileSystemOps, FileSystemOptions, FsNode, FsNodeHandle,
    FsNodeInfo, FsNodeOps, FsStr, FsString, InputBuffer, OutputBuffer, SeekTarget, SymlinkTarget,
    ValueOrSize, default_seek, emit_dotdot, fileops_impl_directory, fileops_impl_noop_sync,
    fileops_impl_seekable, fs_node_impl_dir_readonly, fs_node_impl_not_dir, fs_node_impl_symlink,
};
use anyhow::{Error, anyhow, ensure};
use ext4_metadata::{Metadata, Node, NodeInfo};
use fidl_fuchsia_io as fio;
use starnix_logging::{impossible_error, log_warn, track_stub};
use starnix_sync::{
    DynamicLockDepRwLock, LockDepMutex, LockDepReadGuard, LockDepWriteGuard, RemoteBundleInnerLock,
};
use starnix_types::vfs::default_statfs;
use starnix_uapi::auth::FsCred;
use starnix_uapi::errors::{Errno, SourceContext};
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::mount_flags::FileSystemFlags;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{errno, error, from_status_like_fdio, off_t, statfs};
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use syncio::{zxio_node_attr_has_t, zxio_node_attributes_t};

const REMOTE_BUNDLE_NODE_LRU_CAPACITY: usize = 1024;

/// RemoteBundle is a remote, immutable filesystem that stores additional metadata that would
/// otherwise not be available.  The metadata exists in the "metadata.v1" file, which contains
/// directory, symbolic link and extended attribute information.  Only the content for files are
/// accessed remotely as normal.
pub struct RemoteBundle {
    metadata: Metadata,
    root: fio::DirectorySynchronousProxy,
    rights: fio::Flags,
}

impl RemoteBundle {
    /// Returns a new RemoteBundle filesystem whose path is looked up in the incoming namespace.
    pub fn new_fs(
        current_task: &CurrentTask,
        mut options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        let kernel = current_task.kernel();
        let requested_path = std::str::from_utf8(&options.source)
            .map_err(|_| errno!(EINVAL, "source path is not utf8"))?;
        let (root_proxy, subdir) =
            kernel.open_ns_dir(requested_path, fio::Flags::PROTOCOL_DIRECTORY)?;
        options.source = subdir.into();
        Self::new_fs_in_base(
            current_task.kernel(),
            &root_proxy,
            options,
            fio::PERM_READABLE | fio::PERM_EXECUTABLE,
        )
        .map_err(|e| errno!(EIO, format!("failed to mount remote bundle: {e}")))
    }

    /// Returns a new RemoteBundle filesystem that can be found at `options.source` relative to `base`.
    pub fn new_fs_in_base(
        kernel: &Kernel,
        base: &fio::DirectorySynchronousProxy,
        options: FileSystemOptions,
        rights: fio::Flags,
    ) -> Result<FileSystemHandle, Error> {
        let (root, server_end) = fidl::endpoints::create_sync_proxy::<fio::DirectoryMarker>();
        let path =
            std::str::from_utf8(&options.source).map_err(|_| anyhow!("Source path is not utf8"))?;
        base.open(path, rights, &Default::default(), server_end.into_channel())
            .map_err(|e| anyhow!("Failed to open root: {}", e))?;

        let metadata = {
            let (file, server_end) = fidl::endpoints::create_endpoints::<fio::FileMarker>();
            root.open(
                "metadata.v1",
                fio::PERM_READABLE,
                &Default::default(),
                server_end.into_channel(),
            )
            .source_context("open metadata file")?;
            let mut file: std::fs::File = fdio::create_fd(file.into_channel().into_handle())
                .source_context("create fd from metadata file (wrong mount path?)")?
                .into();
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).source_context("read metadata file")?;
            Metadata::deserialize(&buf).source_context("deserialize metadata file")?
        };

        // Make sure the root node exists.
        ensure!(
            metadata.get(ext4_metadata::ROOT_INODE_NUM).is_some(),
            "Root node does not exist in remote bundle"
        );

        if !rights.contains(fio::PERM_WRITABLE) {
            options.flags.fetch_or(FileSystemFlags::RDONLY, Ordering::Relaxed);
        }

        let fs = FileSystem::new(
            kernel,
            CacheMode::Cached(CacheConfig { capacity: REMOTE_BUNDLE_NODE_LRU_CAPACITY }),
            RemoteBundle { metadata, root, rights },
            options,
        )?;
        fs.create_root(ext4_metadata::ROOT_INODE_NUM, DirectoryObject);
        Ok(fs)
    }

    // Returns the bundle from the filesystem.  Panics if the filesystem isn't associated with a
    // RemoteBundle.
    fn from_fs(fs: &FileSystem) -> &RemoteBundle {
        fs.downcast_ops::<RemoteBundle>().unwrap()
    }

    // Returns a reference to the node identified by `inode_num`.  Panics if the node is not found
    // so this should only be used if the node is known to exist (e.g. the node must exist after
    // `lookup` has run for the relevant node).
    fn get_node(&self, inode_num: u64) -> &Node {
        self.metadata.get(inode_num).unwrap()
    }

    fn get_xattr(&self, node: &FsNode, name: &FsStr) -> Result<ValueOrSize<FsString>, Errno> {
        let value = &self
            .get_node(node.ino)
            .extended_attributes
            .get(&**name)
            .ok_or_else(|| errno!(ENODATA))?[..];
        Ok(FsString::from(value).into())
    }

    fn list_xattrs(&self, node: &FsNode) -> Result<ValueOrSize<Vec<FsString>>, Errno> {
        Ok(self
            .get_node(node.ino)
            .extended_attributes
            .keys()
            .map(|k| FsString::from(&k[..]))
            .collect::<Vec<_>>()
            .into())
    }
}

impl FileSystemOps for RemoteBundle {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        const REMOTE_BUNDLE_FS_MAGIC: u32 = u32::from_be_bytes(*b"bndl");
        let mut stats = default_statfs(REMOTE_BUNDLE_FS_MAGIC);
        // Provide a placeholder non-zero block count to satisfy CTS StatFsTest.
        // 100,000 blocks (approx 400MB with 4KB page size) is safe and adequate
        // to prevent other applications from thinking the disk is full/too small.
        track_stub!(TODO("https://fxbug.dev/540853536"), "RemoteBundle statfs block count");
        stats.f_blocks = 100000;
        Ok(stats)
    }
    fn name(&self) -> &'static FsStr {
        "remote_bundle".into()
    }
    fn update_flags(
        &self,
        fs: &FileSystem,
        _current_task: &CurrentTask,
        flags: FileSystemFlags,
    ) -> Result<(), Errno> {
        fs.options.flags.store(flags | FileSystemFlags::RDONLY, Ordering::Relaxed);
        Ok(())
    }
}

struct File {
    inner: LockDepMutex<Inner, RemoteBundleInnerLock>,
}

enum Inner {
    NeedsVmo(fio::FileSynchronousProxy),
    Memory(Arc<MemoryObject>),
}

impl Inner {
    fn get_memory(&mut self) -> Result<Arc<MemoryObject>, Errno> {
        if let Inner::NeedsVmo(file) = &*self {
            let memory = Arc::new(MemoryObject::from(
                file.get_backing_memory(fio::VmoFlags::READ, zx::MonotonicInstant::INFINITE)
                    .map_err(|err| errno!(EIO, format!("Error {err} on GetBackingMemory")))?
                    .map_err(|s| from_status_like_fdio!(zx::Status::err_from_raw(s)))?,
            ));
            *self = Inner::Memory(memory);
        }
        let Inner::Memory(memory) = &*self else { unreachable!() };
        Ok(memory.clone())
    }
}

impl FsNodeOps for File {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        let memory = self.inner.lock().get_memory()?;
        let size = usize::try_from(memory.get_content_size()).unwrap();
        Ok(Box::new(MemoryFile { memory, size }))
    }

    fn fetch_and_refresh_info<'a>(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        info: &'a DynamicLockDepRwLock<FsNodeInfo>,
    ) -> Result<LockDepReadGuard<'a, FsNodeInfo>, Errno> {
        let memory = self.inner.lock().get_memory()?;
        let content_size = memory.get_content_size();
        let attrs = zxio_node_attributes_t {
            content_size: content_size,
            // TODO(https://fxbug.dev/293607051): Plumb through storage size from underlying connection.
            storage_size: content_size,
            link_count: 1,
            has: zxio_node_attr_has_t {
                content_size: true,
                storage_size: true,
                link_count: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut info = info.write();
        update_info_from_attrs(&mut info, &attrs);
        Ok(LockDepWriteGuard::downgrade(info))
    }

    fn get_xattr(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
        _size: usize,
    ) -> Result<ValueOrSize<FsString>, Errno> {
        let fs = node.fs();
        let bundle = RemoteBundle::from_fs(&fs);
        bundle.get_xattr(node, name)
    }

    fn list_xattrs(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        _size: usize,
    ) -> Result<ValueOrSize<Vec<FsString>>, Errno> {
        let fs = node.fs();
        let bundle = RemoteBundle::from_fs(&fs);
        bundle.list_xattrs(node)
    }
}

// NB: This is different from MemoryRegularFile, which is designed to wrap a VMO that is owned and
// managed by Starnix.  This struct is a wrapper around a pager-backed VMO received from the
// filesystem backing the remote bundle.
// MemoryRegularFile does its own content size management, which is (a) incompatible with the content
// size management done for us by the remote filesystem, and (b) the content size is based on file
// attributes in the case of MemoryRegularFile, which we've intentionally avoided querying here for
// performance.  Specifically, MemoryFile is designed to be opened as fast as possible, and requiring
// that we stat the file whilst opening it is counter to that goal.
// Note that MemoryFile assumes that the underlying file is read-only and not resizable (which is the
// case for remote bundles since they're stored as blobs).
struct MemoryFile {
    memory: Arc<MemoryObject>,
    size: usize,
}

impl FileOps for MemoryFile {
    fileops_impl_seekable!();
    fileops_impl_noop_sync!();

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        mut offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        data.write_each(&mut |buf| {
            let buflen = buf.len();
            let buf = &mut buf[..std::cmp::min(self.size.saturating_sub(offset), buflen)];
            if !buf.is_empty() {
                self.memory
                    .read_uninit(buf, offset as u64)
                    .map_err(|status| from_status_like_fdio!(status))?;
                offset += buf.len();
            }
            Ok(buf.len())
        })
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        error!(EPERM)
    }

    fn get_memory(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _length: Option<usize>,
        prot: ProtectionFlags,
    ) -> Result<Arc<MemoryObject>, Errno> {
        Ok(if prot.contains(ProtectionFlags::EXEC) {
            Arc::new(
                self.memory
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .map_err(impossible_error)?
                    .replace_as_executable(&VMEX_RESOURCE)
                    .map_err(impossible_error)?,
            )
        } else {
            self.memory.clone()
        })
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _waiter: &Waiter,
        _events: FdEvents,
        _handler: EventHandler,
    ) -> Option<WaitCanceler> {
        None
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        Ok(FdEvents::POLLIN)
    }
}

struct DirectoryObject;

impl FileOps for DirectoryObject {
    fileops_impl_directory!();
    fileops_impl_noop_sync!();

    fn seek(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        current_offset: off_t,
        target: SeekTarget,
    ) -> Result<off_t, Errno> {
        default_seek(current_offset, target, || error!(EINVAL))
    }

    fn readdir(
        &self,
        file: &FileObject,
        _current_task: &CurrentTask,
        sink: &mut dyn DirentSink,
    ) -> Result<(), Errno> {
        emit_dotdot(file, sink)?;

        let bundle = RemoteBundle::from_fs(&file.fs);
        let child_iter = bundle
            .get_node(file.node().ino)
            .directory()
            .ok_or_else(|| errno!(EIO))?
            .children
            .iter();

        for (name, inode_num) in child_iter.skip(sink.offset() as usize - 2) {
            let node = bundle.metadata.get(*inode_num).ok_or_else(|| errno!(EIO))?;
            sink.add(
                *inode_num,
                sink.offset() + 1,
                DirectoryEntryType::from_mode(FileMode::from_bits(node.mode.into())),
                name.as_str().into(),
            )?;
        }

        Ok(())
    }
}

impl FsNodeOps for DirectoryObject {
    fs_node_impl_dir_readonly!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(DirectoryObject))
    }

    fn lookup(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
    ) -> Result<FsNodeHandle, Errno> {
        let name = std::str::from_utf8(name).map_err(|_| {
            log_warn!("bad utf8 in pathname! remote filesystems can't handle this");
            errno!(EINVAL)
        })?;

        let fs = node.fs();
        let bundle = RemoteBundle::from_fs(&fs);
        let metadata = &bundle.metadata;
        let ino = metadata
            .lookup(node.ino, name)
            .map_err(|e| errno!(ENOENT, format!("Error: {e:?} opening {name}")))?;
        let metadata_node = metadata.get(ino).ok_or_else(|| errno!(EIO))?;
        let info = to_fs_node_info(metadata_node);

        match metadata_node.info() {
            NodeInfo::Symlink(_) => Ok(fs.create_node(ino, SymlinkObject, info)),
            NodeInfo::Directory(_) => Ok(fs.create_node(ino, DirectoryObject, info)),
            NodeInfo::File(_) => {
                let (file, server_end) = fidl::endpoints::create_sync_proxy::<fio::FileMarker>();
                bundle
                    .root
                    .open(
                        &format!("{ino}"),
                        bundle.rights,
                        &Default::default(),
                        server_end.into_channel(),
                    )
                    .map_err(|_| errno!(EIO))?;
                Ok(fs.create_node(ino, File { inner: Inner::NeedsVmo(file).into() }, info))
            }
        }
    }

    fn get_xattr(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
        _size: usize,
    ) -> Result<ValueOrSize<FsString>, Errno> {
        let fs = node.fs();
        let bundle = RemoteBundle::from_fs(&fs);
        bundle.get_xattr(node, name)
    }

    fn list_xattrs(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        _size: usize,
    ) -> Result<ValueOrSize<Vec<FsString>>, Errno> {
        let fs = node.fs();
        let bundle = RemoteBundle::from_fs(&fs);
        bundle.list_xattrs(node)
    }
}

struct SymlinkObject;

impl FsNodeOps for SymlinkObject {
    fs_node_impl_symlink!();

    fn readlink(&self, node: &FsNode, _current_task: &CurrentTask) -> Result<SymlinkTarget, Errno> {
        let fs = node.fs();
        let bundle = RemoteBundle::from_fs(&fs);
        let target = bundle.get_node(node.ino).symlink().ok_or_else(|| errno!(EIO))?.target.clone();
        Ok(SymlinkTarget::Path(target.as_str().into()))
    }

    fn get_xattr(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        name: &FsStr,
        _size: usize,
    ) -> Result<ValueOrSize<FsString>, Errno> {
        let fs = node.fs();
        let bundle = RemoteBundle::from_fs(&fs);
        bundle.get_xattr(node, name)
    }

    fn list_xattrs(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        _size: usize,
    ) -> Result<ValueOrSize<Vec<FsString>>, Errno> {
        let fs = node.fs();
        let bundle = RemoteBundle::from_fs(&fs);
        bundle.list_xattrs(node)
    }
}

fn to_fs_node_info(metadata_node: &ext4_metadata::Node) -> FsNodeInfo {
    let mode = FileMode::from_bits(metadata_node.mode.into());
    let owner = FsCred { uid: metadata_node.uid.into(), gid: metadata_node.gid.into() };
    let mut info = FsNodeInfo::new(mode, owner);
    // Set the information for directory and links. For file, they will be overwritten
    // by the FsNodeOps on first access.
    // For now, we just use some made up values. We might need to revisit this.
    info.size = 1;
    info.blocks = 1;
    info.blksize = DEFAULT_BYTES_PER_BLOCK;
    info.link_count = 1;
    info
}

#[cfg(test)]
mod test {
    use crate::fs::fuchsia::RemoteBundle;
    use crate::testing::spawn_kernel_and_run_with_pkgfs;
    use crate::vfs::buffers::VecOutputBuffer;
    use crate::vfs::{
        DirectoryEntryType, DirentSink, FileSystemOptions, FsStr, LookupContext, Namespace,
        SymlinkMode, SymlinkTarget,
    };
    use fidl_fuchsia_io as fio;
    use starnix_uapi::errors::Errno;
    use starnix_uapi::file_mode::FileMode;
    use starnix_uapi::open_flags::OpenFlags;
    use starnix_uapi::{ino_t, off_t};
    use std::collections::{HashMap, HashSet};
    use zx;

    #[::fuchsia::test]
    async fn test_read_image() {
        spawn_kernel_and_run_with_pkgfs(async |current_task| {
            let kernel = current_task.kernel();
            let rights = fio::PERM_READABLE | fio::PERM_EXECUTABLE;
            let (server, client) = zx::Channel::create();
            fdio::open("/pkg", rights, server).expect("failed to open /pkg");
            let fs = RemoteBundle::new_fs_in_base(
                &kernel,
                &fio::DirectorySynchronousProxy::new(client),
                FileSystemOptions { source: "data/test-image".into(), ..Default::default() },
                rights,
            )
            .expect("new_fs failed");
            let ns = Namespace::new(fs);
            let root = ns.root();
            let mut context = LookupContext::new(SymlinkMode::NoFollow);

            let test_dir = root
                .lookup_child(&current_task, &mut context, "foo".into())
                .expect("lookup failed");

            let test_file = test_dir
                .lookup_child(&current_task, &mut context, "file".into())
                .expect("lookup failed")
                .open(&current_task, OpenFlags::RDONLY)
                .expect("open failed");

            let mut buffer = VecOutputBuffer::new(64);
            assert_eq!(test_file.read(&current_task, &mut buffer).expect("read failed"), 6);
            let buffer: Vec<u8> = buffer.into();
            assert_eq!(&buffer[..6], b"hello\n");

            assert_eq!(
                &test_file
                    .node()
                    .get_xattr(&current_task, &test_dir.mount, "user.a".into(), usize::MAX)
                    .expect("get_xattr failed")
                    .unwrap(),
                "apple"
            );
            assert_eq!(
                &test_file
                    .node()
                    .get_xattr(&current_task, &test_dir.mount, "user.b".into(), usize::MAX)
                    .expect("get_xattr failed")
                    .unwrap(),
                "ball"
            );
            assert_eq!(
                test_file
                    .node()
                    .list_xattrs(&current_task, usize::MAX)
                    .expect("list_xattr failed")
                    .unwrap()
                    .into_iter()
                    .collect::<HashSet<_>>(),
                ["user.a".into(), "user.b".into()].into_iter().collect::<HashSet<_>>(),
            );

            {
                let info = test_file.node().info();
                assert_eq!(info.mode, FileMode::from_bits(0o100640));
                assert_eq!(info.uid, 49152); // These values come from the test image generated in
                assert_eq!(info.gid, 24403); // ext4_to_pkg.
            }

            let test_symlink = test_dir
                .lookup_child(&current_task, &mut context, "symlink".into())
                .expect("lookup failed");

            if let SymlinkTarget::Path(target) =
                test_symlink.readlink(&current_task).expect("readlink failed")
            {
                assert_eq!(&target, "file");
            } else {
                panic!("unexpected symlink type");
            }

            let opened_dir = test_dir.open(&current_task, OpenFlags::RDONLY).expect("open failed");

            struct Sink {
                offset: off_t,
                entries: HashMap<Vec<u8>, (ino_t, DirectoryEntryType)>,
            }

            impl DirentSink for Sink {
                fn add(
                    &mut self,
                    inode_num: ino_t,
                    offset: off_t,
                    entry_type: DirectoryEntryType,
                    name: &FsStr,
                ) -> Result<(), Errno> {
                    assert_eq!(offset, self.offset + 1);
                    self.entries.insert(name.to_vec(), (inode_num, entry_type));
                    self.offset = offset;
                    Ok(())
                }

                fn offset(&self) -> off_t {
                    self.offset
                }
            }

            let mut sink = Sink { offset: 0, entries: HashMap::new() };
            opened_dir.readdir(&current_task, &mut sink).expect("readdir failed");

            assert_eq!(
                sink.entries,
                [
                    (b".".into(), (test_dir.entry.node.ino, DirectoryEntryType::DIR)),
                    (b"..".into(), (root.entry.node.ino, DirectoryEntryType::DIR)),
                    (b"file".into(), (test_file.node().ino, DirectoryEntryType::REG)),
                    (b"symlink".into(), (test_symlink.entry.node.ino, DirectoryEntryType::LNK))
                ]
                .into()
            );
        })
        .await;
    }
}
