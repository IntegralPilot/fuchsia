// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://github.com/rust-lang/rust/issues/39371): remove
#![allow(non_upper_case_globals)]

use crate::bpf::syscalls::BpfTypeFormat;
use crate::bpf::{BpfMapHandle, ProgramHandle};
use crate::mm::memory::MemoryObject;
use crate::mm::{DesiredAddress, MappingOptions, ProtectionFlags};
use crate::security::{self, PermissionFlags};
use crate::task::{
    CurrentTask, EventHandler, SignalHandler, SignalHandlerInner, Task, WaitCanceler, Waiter,
};
use crate::vfs::buffers::{InputBuffer, OutputBuffer};
use crate::vfs::{
    AccessCheck, CacheMode, FdNumber, FileObject, FileOps, FileSystem, FileSystemHandle,
    FileSystemOps, FileSystemOptions, FsNode, FsNodeHandle, FsNodeInfo, FsNodeOps, FsStr,
    MemoryDirectoryFile, MemoryXattrStorage, NamespaceNode, RenameContext, XattrStorage as _,
    default_mmap, fileops_impl_nonseekable, fileops_impl_noop_sync, fs_node_impl_not_dir,
    fs_node_impl_xattr_delegate,
};
use bstr::BStr;
use ebpf_api::RINGBUF_SIGNAL;
use starnix_logging::track_stub;
use starnix_types::vfs::default_statfs;
use starnix_uapi::auth::FsCred;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::{FileMode, mode};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::UserAddress;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{BPF_FS_MAGIC, bpf_map_type_BPF_MAP_TYPE_RINGBUF, errno, error, statfs};
use std::sync::Arc;

/// A reference to a BPF object that can be stored in either an FD or an entry in the /sys/fs/bpf
/// filesystem.
#[derive(Debug, Clone)]
pub enum BpfHandle {
    Program(ProgramHandle),

    // Stub used to fake loading of programs of unknown types.
    ProgramStub(u32),

    Map(BpfMapHandle),
    BpfTypeFormat(Arc<BpfTypeFormat>),
}

impl BpfHandle {
    pub fn as_map(&self) -> Result<&BpfMapHandle, Errno> {
        match self {
            Self::Map(map) => Ok(map),
            _ => error!(EINVAL),
        }
    }
    pub fn as_program(&self) -> Result<&ProgramHandle, Errno> {
        match self {
            Self::Program(program) => Ok(program),
            _ => error!(EINVAL),
        }
    }

    pub fn into_program(self) -> Result<ProgramHandle, Errno> {
        match self {
            Self::Program(program) => Ok(program),
            _ => error!(EINVAL),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Map(_) => "bpf-map",
            Self::Program(_) | Self::ProgramStub(_) => "bpf-prog",
            Self::BpfTypeFormat(_) => "bpf-type",
        }
    }

    /// Performs security-related checks when opening a BPF map. If
    /// `permission_flags` is `None`, then they are inferred from the map's
    /// schema. `permission_flags` is ignored for programs.
    pub(super) fn security_check_open_fd(
        &self,
        current_task: &CurrentTask,
        permission_flags: Option<PermissionFlags>,
    ) -> Result<(), Errno> {
        match self {
            Self::Map(bpf_map) => security::check_bpf_map_access(
                current_task,
                &bpf_map.security_state,
                permission_flags.unwrap_or_else(|| bpf_map.schema.flags.into()),
            ),
            Self::Program(program) => {
                security::check_bpf_prog_access(current_task, &program.security_state)
            }
            _ => Ok(()),
        }
    }
}

impl From<ProgramHandle> for BpfHandle {
    fn from(program: ProgramHandle) -> Self {
        Self::Program(program)
    }
}

impl From<BpfMapHandle> for BpfHandle {
    fn from(map: BpfMapHandle) -> Self {
        Self::Map(map)
    }
}

impl From<BpfTypeFormat> for BpfHandle {
    fn from(format: BpfTypeFormat) -> Self {
        Self::BpfTypeFormat(Arc::new(format))
    }
}

impl FileOps for BpfHandle {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();
    fn read(
        &self,
        _file: &FileObject,
        _current_task: &crate::task::CurrentTask,
        _offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        track_stub!(TODO("https://fxbug.dev/322874229"), "bpf handle read");
        error!(EINVAL)
    }
    fn write(
        &self,
        _file: &FileObject,
        _current_task: &crate::task::CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        track_stub!(TODO("https://fxbug.dev/322873841"), "bpf handle write");
        error!(EINVAL)
    }

    fn get_memory(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        length: Option<usize>,
        prot: ProtectionFlags,
    ) -> Result<Arc<MemoryObject>, Errno> {
        // Because of the specific condition needed to map this object, the size must be known.
        let length = length.ok_or_else(|| errno!(EINVAL))?;

        // This cannot be mapped executable.
        if prot.contains(ProtectionFlags::EXEC) {
            return error!(EPERM);
        }

        self.as_map()?.get_memory(length, prot)
    }

    fn mmap(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        addr: DesiredAddress,
        memory_offset: u64,
        length: usize,
        prot_flags: ProtectionFlags,
        options: MappingOptions,
    ) -> Result<UserAddress, Errno> {
        let BpfHandle::Map(bpf_map) = &self else {
            return error!(EINVAL);
        };
        security::check_bpf_map_access(
            current_task,
            &bpf_map.security_state,
            PermissionFlags::READ | PermissionFlags::WRITE,
        )?;
        let options = options | MappingOptions::DONT_EXPAND;
        default_mmap(file, current_task, addr, memory_offset, length, prot_flags, options)
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        let BpfHandle::Map(bpf_map) = self else {
            return None;
        };

        // Only ringbuffers can be polled for POLLIN.
        if bpf_map.schema.map_type != bpf_map_type_BPF_MAP_TYPE_RINGBUF
            || !events.contains(FdEvents::POLLIN)
        {
            return Some(WaitCanceler::new_noop());
        }

        let handler = SignalHandler {
            inner: SignalHandlerInner::ZxHandle(|signals| {
                if signals.contains(RINGBUF_SIGNAL) { FdEvents::POLLIN } else { FdEvents::empty() }
            }),
            event_handler: handler,
            err_code: None,
        };

        // Reset the signal before waiting. The case when the ring buffer already has some data
        // is handled by the caller: it should call `query_events` after starting the waiter.
        bpf_map
            .vmo()
            .as_handle_ref()
            .signal(RINGBUF_SIGNAL, zx::Signals::empty())
            .expect("Failed to set signal or a ring buffer VMO");

        let canceler = waiter
            .wake_on_zircon_signals(&bpf_map.vmo().as_handle_ref(), RINGBUF_SIGNAL, handler)
            .expect("Failed to wait for signals on ringbuf VMO");
        Some(WaitCanceler::new_port(canceler))
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        match self {
            Self::Map(map) => {
                let events = match map.can_read() {
                    Some(true) => FdEvents::POLLIN,
                    Some(false) => FdEvents::empty(),
                    None => FdEvents::POLLERR,
                };
                Ok(events)
            }
            _ => error!(EPERM),
        }
    }
}

pub fn get_bpf_object(task: &Task, fd: FdNumber) -> Result<BpfHandle, Errno> {
    Ok((*task.files()?.get(fd)?.downcast_file::<BpfHandle>().ok_or_else(|| errno!(EBADF))?).clone())
}
pub struct BpfFs;
impl BpfFs {
    pub fn new_fs(
        current_task: &CurrentTask,
        options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        let kernel = current_task.kernel();
        let fs = FileSystem::new(kernel, CacheMode::Permanent, BpfFs, options)?;
        let root_ino = fs.allocate_ino();
        fs.create_root_with_info(
            root_ino,
            BpfFsDir::new(),
            FsNodeInfo::new(mode!(IFDIR, 0o777) | FileMode::ISVTX, FsCred::root()),
        );
        Ok(fs)
    }
}

impl FileSystemOps for BpfFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(BPF_FS_MAGIC))
    }
    fn name(&self) -> &'static FsStr {
        "bpf".into()
    }

    fn rename(
        &self,
        _fs: &FileSystem,
        _current_task: &CurrentTask,
        _context: &mut RenameContext<'_>,
        _old_name: &FsStr,
        _new_name: &FsStr,
    ) -> Result<(), Errno> {
        Ok(())
    }
}

pub struct BpfFsDir {
    xattrs: MemoryXattrStorage,
}

impl BpfFsDir {
    fn new() -> Self {
        Self { xattrs: MemoryXattrStorage::default() }
    }

    pub fn register_pin(
        &self,
        current_task: &CurrentTask,
        node: &NamespaceNode,
        name: &FsStr,
        object: BpfHandle,
    ) -> Result<(), Errno> {
        node.entry.create_entry(current_task, &node.mount, name, |dir, _mount, _name| {
            Ok(dir.fs().create_node_and_allocate_node_id(
                BpfFsObject::new(object),
                FsNodeInfo::new(mode!(IFREG, 0o600), current_task.current_fscred()),
            ))
        })?;
        Ok(())
    }
}

impl FsNodeOps for BpfFsDir {
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
        Ok(node.fs().create_node_and_allocate_node_id(
            BpfFsDir::new(),
            FsNodeInfo::new(mode | FileMode::ISVTX, owner),
        ))
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
        error!(EPERM)
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

    fn link(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _child: &FsNodeHandle,
    ) -> Result<(), Errno> {
        Ok(())
    }

    fn unlink(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _child: &FsNodeHandle,
    ) -> Result<(), Errno> {
        Ok(())
    }
}

pub struct BpfFsObject {
    pub handle: BpfHandle,
    xattrs: MemoryXattrStorage,
}

impl BpfFsObject {
    fn new(handle: BpfHandle) -> Self {
        Self { handle, xattrs: MemoryXattrStorage::default() }
    }
}

impl FsNodeOps for BpfFsObject {
    fs_node_impl_not_dir!();
    fs_node_impl_xattr_delegate!(self, self.xattrs);

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        error!(EIO)
    }
}

/// Resolves a pinned BPF object from a path, returning the underlying handle.
/// Performs DAC and MAC checks using the specified `open_flags `. Also updates
/// atime unless `NOATIME` flag is set.
pub fn resolve_pinned_bpf_object(
    current_task: &CurrentTask,
    path: &BStr,
    open_flags: OpenFlags,
) -> Result<BpfHandle, Errno> {
    let node = current_task.lookup_path_from_root(path.as_ref())?;

    let permission_flags = PermissionFlags::from(open_flags);
    node.check_access(current_task, AccessCheck::for_access(permission_flags))?;

    let object = node.entry.node.downcast_ops::<BpfFsObject>().ok_or_else(|| errno!(EPERM))?;
    object.handle.security_check_open_fd(current_task, Some(permission_flags))?;

    if !open_flags.contains(OpenFlags::NOATIME) {
        node.update_atime();
    }

    Ok(object.handle.clone())
}
