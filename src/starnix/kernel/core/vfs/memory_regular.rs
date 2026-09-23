// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fs::tmpfs::TmpFs;
use crate::mm::memory::MemoryObject;
use crate::mm::{PAGE_SIZE, ProtectionFlags, VMEX_RESOURCE};
use crate::security;
use crate::signals::{SignalInfo, send_standard_signal};
use crate::task::CurrentTask;
use crate::vfs::buffers::{InputBuffer, OutputBuffer};
use crate::vfs::{
    AppendLockWriteGuard, DirEntry, FallocMode, FileHandle, FileObject, FileOps, FileSystemHandle,
    FsNode, FsNodeInfo, FsNodeLinkBehavior, FsNodeOps, FsString, MAX_LFS_FILESIZE,
    MemoryXattrStorage, Mount, MountInfo, NamespaceNode, OpenAccessCheck, WhatToMount,
    XattrStorage as _, fileops_impl_noop_sync, fs_node_impl_not_dir, fs_node_impl_xattr_delegate,
};
use linux_uapi::{ASHMEM_GET_SIZE, ASHMEM_SET_SIZE};
use starnix_logging::{impossible_error, track_stub};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_types::math::round_up_to_system_page_size;
use starnix_uapi::errors::{EFBIG, Errno};
use starnix_uapi::file_mode::mode;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::resource_limits::Resource;
use starnix_uapi::seal_flags::SealFlags;
use starnix_uapi::signals::SIGXFSZ;
use starnix_uapi::{errno, error};
use std::sync::Arc;

pub struct MemoryRegularNode {
    /// The memory that backs this file.
    memory: Arc<MemoryObject>,
    xattrs: MemoryXattrStorage,
}

impl MemoryRegularNode {
    /// Create a new writable file node based on a blank VMO.
    pub fn new() -> Result<Self, Errno> {
        let vmo =
            zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, 0).map_err(|_| errno!(ENOMEM))?;
        Ok(Self {
            memory: Arc::new(MemoryObject::from(vmo).with_zx_name(b"starnix:vfs")),
            xattrs: MemoryXattrStorage::default(),
        })
    }

    /// Create a new file node based on an existing VMO.
    /// Attempts to open the file for writing will fail unless [`memory`] has both
    /// the `WRITE` and `RESIZE` rights.
    pub fn from_memory(memory: Arc<MemoryObject>) -> Self {
        Self { memory, xattrs: MemoryXattrStorage::default() }
    }
}

impl FsNodeOps for MemoryRegularNode {
    fs_node_impl_not_dir!();
    fs_node_impl_xattr_delegate!(self, self.xattrs);

    fn initial_info(&self, info: &mut FsNodeInfo) {
        info.size = self.memory.get_content_size() as usize;
    }

    fn create_file_ops(
        &self,
        node: &FsNode,
        _current_task: &CurrentTask,
        flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        if flags.contains(OpenFlags::TRUNC) {
            // Truncating to zero length must pass the shrink seal check.
            node.write_guard_state.lock().check_no_seal(SealFlags::SHRINK)?;
        }

        // Produce a VMO handle with rights reduced to those requested in |flags|.
        let mut desired_rights = zx::Rights::VMO_DEFAULT | zx::Rights::RESIZE;
        if !flags.can_read() {
            desired_rights.remove(zx::Rights::READ);
        }
        if !flags.can_write() {
            desired_rights.remove(zx::Rights::WRITE | zx::Rights::RESIZE);
        }
        let scoped_memory =
            Arc::new(self.memory.duplicate_handle(desired_rights).map_err(|_e| errno!(EIO))?);
        let file_object = MemoryRegularFile::new(scoped_memory);

        Ok(Box::new(file_object))
    }

    fn truncate(
        &self,
        _guard: &AppendLockWriteGuard<'_>,
        node: &FsNode,
        _current_task: &CurrentTask,
        length: u64,
    ) -> Result<(), Errno> {
        let length = length as usize;

        node.update_info(|info| {
            if info.size == length {
                // The file size remains unaffected.
                return Ok(());
            }

            // We must hold the lock till the end of the operation to guarantee that
            // there is no change to the seals.
            let state = node.write_guard_state.lock();

            if info.size > length {
                // A decrease in file size must pass the shrink seal check.
                state.check_no_seal(SealFlags::SHRINK)?;
            } else {
                // An increase in file size must pass the grow seal check.
                state.check_no_seal(SealFlags::GROW)?;
            }

            let memory_size = update_memory_file_size(&self.memory, info, length)?;
            info.size = length;

            // Zero unused parts of the VMO.
            if memory_size > length {
                self.memory
                    .op_range(zx::VmoOp::ZERO, length as u64, (memory_size - length) as u64)
                    .map_err(impossible_error)?;
            }

            Ok(())
        })
    }

    fn allocate(
        &self,
        _guard: &AppendLockWriteGuard<'_>,
        node: &FsNode,
        _current_task: &CurrentTask,
        mode: FallocMode,
        offset: u64,
        length: u64,
    ) -> Result<(), Errno> {
        match mode {
            FallocMode::PunchHole => {
                // Lock `info()` before acquiring the `write_guard_state` lock to ensure consistent
                // lock ordering.
                let info = node.info();

                // Check write seal. Hold the lock to ensure seals don't change.
                let state = node.write_guard_state.lock();
                state.check_no_seal(SealFlags::WRITE | SealFlags::FUTURE_WRITE)?;

                let mut end = offset.checked_add(length).ok_or_else(|| errno!(EINVAL))? as usize;

                let memory_size = info.blksize * info.blocks;
                if offset as usize >= memory_size {
                    return Ok(());
                }

                // If punching hole at the end of the file then zero all the
                // way to the end of the VMO to avoid keeping any pages for the tail.
                if end >= info.size {
                    end = memory_size;
                }

                self.memory
                    .op_range(zx::VmoOp::ZERO, offset, end as u64 - offset)
                    .map_err(impossible_error)?;

                Ok(())
            }

            FallocMode::Allocate { keep_size } => {
                node.update_info(|info| {
                    let new_size = (offset + length) as usize;
                    if new_size > info.size {
                        // Check GROW seal (even with `keep_size=true`). Hold the lock to ensure
                        // seals don't change.
                        let state = node.write_guard_state.lock();
                        state.check_no_seal(SealFlags::GROW)?;

                        update_memory_file_size(&self.memory, info, new_size)?;

                        if !keep_size {
                            info.size = new_size;
                        }
                    }
                    Ok(())
                })
            }

            _ => error!(EOPNOTSUPP),
        }
    }
}

pub struct MemoryRegularFile {
    pub memory: Arc<MemoryObject>,
}

impl MemoryRegularFile {
    /// Create a file object based on a VMO.
    pub fn new(memory: Arc<MemoryObject>) -> Self {
        MemoryRegularFile { memory }
    }
}

impl MemoryRegularFile {
    pub fn read(
        memory: &Arc<MemoryObject>,
        file: &FileObject,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        let actual = {
            let info = file.node().info();
            let file_length = info.size;
            let want_read = data.available();
            if offset < file_length {
                let to_read =
                    if file_length < offset + want_read { file_length - offset } else { want_read };
                let buf =
                    memory.read_to_vec(offset as u64, to_read as u64).map_err(|_| errno!(EIO))?;
                drop(info);
                data.write_all(&buf[..])?;
                to_read
            } else {
                0
            }
        };
        Ok(actual)
    }

    pub fn write(
        memory: &Arc<MemoryObject>,
        file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        let mut want_write = data.available();
        let buf = data.peek_all()?;

        let result = file.node().update_info(|info| {
            let mut write_end = offset + want_write;
            let mut update_content_size = false;

            // We must hold the lock till the end of the operation to guarantee that
            // there is no change to the seals.
            let state = file.name.entry.node.write_guard_state.lock();

            // Non-zero writes must pass the write seal check.
            if want_write != 0 {
                state.check_no_seal(SealFlags::WRITE | SealFlags::FUTURE_WRITE)?;
            }

            // Writing past the file size
            if write_end > info.size {
                // The grow seal check failed.
                if let Err(e) = state.check_no_seal(SealFlags::GROW) {
                    if offset >= info.size {
                        // Write starts outside the file.
                        // Forbid because nothing can be written without growing.
                        return Err(e);
                    } else if info.size == info.storage_size() {
                        // Write starts inside file and EOF page does not need to grow.
                        // End write at EOF.
                        write_end = info.size;
                        want_write = write_end - offset;
                    } else {
                        // Write starts inside file and EOF page needs to grow.
                        let eof_page_start = info.storage_size() - (*PAGE_SIZE as usize);

                        if offset >= eof_page_start {
                            // Write starts in EOF page.
                            // Forbid because EOF page cannot grow.
                            return Err(e);
                        }

                        // End write at page before EOF.
                        write_end = eof_page_start;
                        want_write = write_end - offset;
                    }
                }
            }

            // Check against the FSIZE limit
            let fsize_limit = current_task.thread_group().get_rlimit(Resource::FSIZE) as usize;
            if write_end > fsize_limit {
                if offset >= fsize_limit {
                    // Write starts beyond the FSIZE limit.
                    return error!(EFBIG);
                }

                // End write at FSIZE limit.
                write_end = fsize_limit;
                want_write = write_end - offset;
            }

            if write_end > info.size {
                if write_end > info.storage_size() {
                    update_memory_file_size(memory, info, write_end)?;
                }
                update_content_size = true;
            }
            memory.write(&buf[..want_write], offset as u64).map_err(|_| errno!(EIO))?;

            if update_content_size {
                info.size = write_end;
            }
            data.advance(want_write)?;
            Ok(want_write)
        });
        if let Err(ref e) = result
            && *e == EFBIG
        {
            // EFBIG must trigger a signal. Sending the signal must be done outside of the
            // update_info method to ensure no deadlock.
            send_standard_signal(current_task, SignalInfo::kernel(SIGXFSZ));
        }
        result
    }

    pub fn get_memory(
        memory: &Arc<MemoryObject>,
        file: &FileObject,
        _current_task: &CurrentTask,
        prot: ProtectionFlags,
    ) -> Result<Arc<MemoryObject>, Errno> {
        // In MemoryFileNode::create_file_ops, we downscoped the rights
        // on the VMO to match the rights on the file object. If the caller
        // wants more rights than exist on the file object, return an error
        // instead of returning a MemoryObject that does not conform to
        // the FileOps::get_memory contract.
        if prot.contains(ProtectionFlags::READ) && !file.can_read() {
            return error!(EACCES);
        }
        if prot.contains(ProtectionFlags::WRITE) && !file.can_write() {
            return error!(EACCES);
        }
        let mut memory = Arc::clone(memory);
        if prot.contains(ProtectionFlags::EXEC) {
            memory = Arc::new(
                memory
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .map_err(impossible_error)?
                    .replace_as_executable(&VMEX_RESOURCE)
                    .map_err(impossible_error)?,
            );
        }
        Ok(memory)
    }
}

#[macro_export]
macro_rules! fileops_impl_memory {
    ($self:ident, $memory:expr) => {
        $crate::fileops_impl_seekable!();

        fn read(
            &$self,
            file: &$crate::vfs::FileObject,
            _current_task: &$crate::task::CurrentTask,
            offset: usize,
            data: &mut dyn $crate::vfs::buffers::OutputBuffer,
        ) -> Result<usize, starnix_uapi::errors::Errno> {
            $crate::vfs::MemoryRegularFile::read($memory, file, offset, data)
        }

        fn write(
            &$self,
            file: &$crate::vfs::FileObject,
            current_task: &$crate::task::CurrentTask,
            offset: usize,
            data: &mut dyn $crate::vfs::buffers::InputBuffer,
        ) -> Result<usize, starnix_uapi::errors::Errno> {
            $crate::vfs::MemoryRegularFile::write($memory, file, current_task, offset, data)
        }

        fn get_memory(
            &$self,
            file: &$crate::vfs::FileObject,
            current_task: &$crate::task::CurrentTask,
            _length: Option<usize>,
            prot: $crate::mm::ProtectionFlags,
        ) -> Result<Arc<$crate::mm::memory::MemoryObject>, starnix_uapi::errors::Errno> {
            $crate::vfs::MemoryRegularFile::get_memory($memory, file, current_task, prot)
        }
    }
}
pub use fileops_impl_memory;

impl FileOps for MemoryRegularFile {
    fileops_impl_memory!(self, &self.memory);
    fileops_impl_noop_sync!();

    fn readahead(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _length: usize,
    ) -> Result<(), Errno> {
        track_stub!(TODO("https://fxbug.dev/42082608"), "paged VMO readahead");
        Ok(())
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        match request {
            ASHMEM_GET_SIZE => {
                track_stub!(TODO("https://fxbug.dev/389102161"), "ashmem get_size on memfd");
                Ok(self.memory.get_size().into())
            }
            ASHMEM_SET_SIZE => {
                track_stub!(TODO("https://fxbug.dev/389102161"), "ashmem set_size on memfd");
                self.memory.set_size(arg.into()).map_err(|_| errno!(EINVAL))?;
                Ok(SUCCESS)
            }
            _ => error!(ENOTTY),
        }
    }
}

pub fn new_memfd(
    current_task: &CurrentTask,
    mut name: FsString,
    seals: SealFlags,
    flags: OpenFlags,
) -> Result<FileHandle, Errno> {
    struct MemFdTmpfs {
        tmpfs: FileSystemHandle,
        mount: Arc<Mount>,
    }

    let fs = current_task.kernel().expando.get_or_init(|| {
        let tmpfs = TmpFs::new_fs(current_task.kernel());
        security::file_system_resolve_security(&current_task, &tmpfs).expect("resolve fs security");
        let mounts_guard = current_task.kernel().mounts_lock();
        let mount = Mount::new(&mounts_guard, WhatToMount::Fs(tmpfs.clone()), Default::default())
            .expect("create new tempfs mount for memfd");
        MemFdTmpfs { tmpfs, mount }
    });

    // Create the node as a kernel-internal operation, to skip the filesystem access-checks.
    // TODO: https://fxbug.dev/455785957 - Validate whether any access-checks should be performed
    // during "memfd" creation.
    let fs_node = current_task.override_creds(
        security::creds_start_internal_operation(current_task),
        || {
            fs.tmpfs.root().node.create_tmpfile(
                current_task,
                &MountInfo::detached(),
                mode!(IFREG, 0o600),
                current_task.current_fscred(),
                FsNodeLinkBehavior::Disallowed,
            )
        },
    )?;

    // LSM allows security modules to choose to treat mem-FDs as a kind of anonymous inode.
    security::fs_node_init_anon(current_task, &fs_node, "[memfd]")?;

    fs_node.write_guard_state.lock().enable_sealing(seals);

    // memfd instances appear in /proc[pid]/fd as though they are O_TMPFILE files with names of
    // the form "memfd:[name]".
    let mut local_name = FsString::from("memfd:");
    local_name.append(&mut name);
    let dir_entry = DirEntry::new_deleted(fs_node, Some(fs.tmpfs.root().clone()), local_name);
    security::fs_node_init_with_dentry(current_task, &dir_entry)?;

    let name = NamespaceNode::new(fs.mount.clone(), dir_entry);
    name.open(current_task, OpenAccessCheck::skip(flags))
}

/// Sets memory size to `min_size` rounded to whole pages. Returns the new size of the VMO in bytes.
fn update_memory_file_size(
    memory: &MemoryObject,
    node_info: &mut FsNodeInfo,
    requested_size: usize,
) -> Result<usize, Errno> {
    assert!(requested_size <= MAX_LFS_FILESIZE);
    let size = round_up_to_system_page_size(requested_size)?;
    memory.set_size(size as u64).map_err(|status| match status {
        zx::Status::NO_MEMORY => errno!(ENOMEM),
        zx::Status::OUT_OF_RANGE => errno!(ENOMEM),
        _ => impossible_error(status),
    })?;
    node_info.blocks = size / node_info.blksize;
    Ok(size)
}
