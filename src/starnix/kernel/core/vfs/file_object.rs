// Cmpyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::memory::MemoryObject;
use crate::mm::{DesiredAddress, MappingName, MappingOptions, MemoryAccessorExt, ProtectionFlags};
use crate::power::OnWakeOps;
use crate::security;
use crate::task::{
    CurrentTask, EventHandler, Pid, WaitCallback, WaitCanceler, Waiter, register_delayed_release,
};
use crate::vfs::buffers::{InputBuffer, OutputBuffer};
use crate::vfs::file_server::serve_file;
use crate::vfs::fsverity::{
    FsVerityState, {self},
};
use crate::vfs::{
    ActiveNamespaceNode, DirentSink, EpollFileObject, EpollKey, FallocMode, FdTableId, FileMapping,
    FileSystemHandle, FileWriteGuardMode, FsNodeHandle, FsString, NamespaceNode, RecordLockCommand,
    RecordLockOwner,
};
use starnix_crypt::EncryptionKeyId;
use starnix_lifecycle::{ObjectReleaser, ReleaserAction};
use starnix_rcu::RcuAtomic;
use starnix_types::ownership::ReleaseGuard;
use starnix_uapi::mount_flags::MountFlags;
use starnix_uapi::user_address::ArchSpecific;

use fidl::endpoints::ProtocolMarker as _;
use linux_uapi::{FSCRYPT_MODE_AES_256_CTS, FSCRYPT_MODE_AES_256_XTS};
use starnix_logging::{CATEGORY_STARNIX_MM, impossible_error, log_error, track_stub};
use starnix_sync::{
    FileAsyncOwnerLock, FileEpollFilesLock, FileLeaseLock, FileObjectOffset, LockDepMutex,
};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_types::math::round_up_to_system_page_size;
use starnix_types::ownership::Releasable;
use starnix_uapi::arc_key::WeakKey;
use starnix_uapi::as_any::AsAny;
use starnix_uapi::auth::{CAP_FOWNER, CAP_SYS_RAWIO};
use starnix_uapi::errors::{EAGAIN, ETIMEDOUT, Errno};
use starnix_uapi::file_lease::FileLeaseType;
use starnix_uapi::file_mode::Access;
use starnix_uapi::inotify_mask::InotifyMask;
use starnix_uapi::open_flags::{AtomicOpenFlags, OpenFlags};
use starnix_uapi::seal_flags::SealFlags;
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{
    F_OWNER_PGRP, F_OWNER_PID, F_OWNER_TID, FIBMAP, FIGETBSZ, FIONBIO, FIONREAD, FIOQSIZE,
    FS_CASEFOLD_FL, FS_IOC_ADD_ENCRYPTION_KEY, FS_IOC_ENABLE_VERITY, FS_IOC_FSGETXATTR,
    FS_IOC_FSSETXATTR, FS_IOC_MEASURE_VERITY, FS_IOC_READ_VERITY_METADATA,
    FS_IOC_REMOVE_ENCRYPTION_KEY, FS_IOC_SET_ENCRYPTION_POLICY, FS_VERITY_FL,
    FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER, FSCRYPT_POLICY_V2, SEEK_CUR, SEEK_DATA, SEEK_END, SEEK_HOLE,
    SEEK_SET, errno, error, fscrypt_add_key_arg, fscrypt_identifier, fsxattr, off_t, pid_t, uapi,
};
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

pub const MAX_LFS_FILESIZE: usize = 0x7fff_ffff_ffff_ffff;

pub fn checked_add_offset_and_length(offset: usize, length: usize) -> Result<usize, Errno> {
    let end = offset.checked_add(length).ok_or_else(|| errno!(EINVAL))?;
    if end > MAX_LFS_FILESIZE {
        return error!(EINVAL);
    }
    Ok(end)
}

#[derive(Debug)]
pub enum SeekTarget {
    /// Seek to the given offset relative to the start of the file.
    Set(off_t),
    /// Seek to the given offset relative to the current position.
    Cur(off_t),
    /// Seek to the given offset relative to the end of the file.
    End(off_t),
    /// Seek for the first data after the given offset,
    Data(off_t),
    /// Seek for the first hole after the given offset,
    Hole(off_t),
}

impl SeekTarget {
    pub fn from_raw(whence: u32, offset: off_t) -> Result<SeekTarget, Errno> {
        match whence {
            SEEK_SET => Ok(SeekTarget::Set(offset)),
            SEEK_CUR => Ok(SeekTarget::Cur(offset)),
            SEEK_END => Ok(SeekTarget::End(offset)),
            SEEK_DATA => Ok(SeekTarget::Data(offset)),
            SEEK_HOLE => Ok(SeekTarget::Hole(offset)),
            _ => error!(EINVAL),
        }
    }

    pub fn whence(&self) -> u32 {
        match self {
            Self::Set(_) => SEEK_SET,
            Self::Cur(_) => SEEK_CUR,
            Self::End(_) => SEEK_END,
            Self::Data(_) => SEEK_DATA,
            Self::Hole(_) => SEEK_HOLE,
        }
    }

    pub fn offset(&self) -> off_t {
        match self {
            Self::Set(off)
            | Self::Cur(off)
            | Self::End(off)
            | Self::Data(off)
            | Self::Hole(off) => *off,
        }
    }
}

/// Corresponds to struct file_operations in Linux, plus any filesystem-specific data.
pub trait FileOps: Send + Sync + AsAny + 'static {
    /// Called when the FileObject is opened/created
    fn open(&self, _file: &FileObject, _current_task: &CurrentTask) -> Result<(), Errno> {
        Ok(())
    }

    /// Called when the FileObject is destroyed.
    fn close(self: Box<Self>, _file: &FileObjectState, _current_task: &CurrentTask) {}

    /// Called every time close() is called on this file, even if the file is not ready to be
    /// released.
    fn flush(&self, _file: &FileObject, _current_task: &CurrentTask) {}

    /// Returns whether the file has meaningful seek offsets. Returning `false` is only
    /// optimization and will makes `FileObject` never hold the offset lock when calling `read` and
    /// `write`.
    fn has_persistent_offsets(&self) -> bool {
        self.is_seekable()
    }

    /// Returns whether the file is seekable.
    fn is_seekable(&self) -> bool;

    /// Returns true if `write()` operations on the file will update the seek offset.
    fn writes_update_seek_offset(&self) -> bool {
        self.has_persistent_offsets()
    }

    /// Read from the file at an offset. If the file does not have persistent offsets (either
    /// directly, or because it is not seekable), offset will be 0 and can be ignored.
    /// Returns the number of bytes read.
    fn read(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno>;

    /// Write to the file with an offset. If the file does not have persistent offsets (either
    /// directly, or because it is not seekable), offset will be 0 and can be ignored.
    /// Returns the number of bytes written.
    fn write(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno>;

    /// Adjust the `current_offset` if the file is seekable.
    fn seek(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        current_offset: off_t,
        target: SeekTarget,
    ) -> Result<off_t, Errno>;

    /// Syncs cached state associated with the file descriptor to persistent storage.
    ///
    /// The method blocks until the synchronization is complete.
    fn sync(&self, file: &FileObject, current_task: &CurrentTask) -> Result<(), Errno> {
        file.node().ops().sync(file.node(), current_task)
    }

    /// Syncs cached data, and only enough metadata to retrieve said data, to persistent storage.
    ///
    /// The method blocks until the synchronization is complete.
    fn data_sync(&self, file: &FileObject, current_task: &CurrentTask) -> Result<(), Errno> {
        // TODO(https://fxbug.dev/297305634) make a default macro once data can be done separately
        self.sync(file, current_task)
    }

    /// Returns a VMO representing this file. At least the requested protection flags must
    /// be set on the VMO. Reading or writing the VMO must read or write the file. If this is not
    /// possible given the requested protection, an error must be returned.
    /// The `length` is a hint for the desired size of the VMO. The returned VMO may be larger or
    /// smaller than the requested length.
    /// This method is typically called by [`Self::mmap`].
    fn get_memory(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _length: Option<usize>,
        _prot: ProtectionFlags,
    ) -> Result<Arc<MemoryObject>, Errno> {
        error!(ENODEV)
    }

    /// Responds to an mmap call. The default implementation calls [`Self::get_memory`] to get a VMO
    /// and then maps it with [`crate::mm::MemoryManager::map`].
    /// Only implement this trait method if your file needs to control mapping, or record where
    /// a VMO gets mapped.
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
        default_mmap(file, current_task, addr, memory_offset, length, prot_flags, options)
    }

    /// Respond to a `getdents` or `getdents64` calls.
    ///
    /// The `file.offset` lock will be held while entering this method. The implementation must look
    /// at `sink.offset()` to read the current offset into the file.
    fn readdir(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _sink: &mut dyn DirentSink,
    ) -> Result<(), Errno> {
        error!(ENOTDIR)
    }

    /// Establish a one-shot, edge-triggered, asynchronous wait for the given FdEvents for the
    /// given file and task. Returns `None` if this file does not support blocking waits.
    ///
    /// Active events are not considered. This is similar to the semantics of the
    /// ZX_WAIT_ASYNC_EDGE flag on zx_wait_async. To avoid missing events, the caller must call
    /// query_events after calling this.
    ///
    /// If your file does not support blocking waits, leave this as the default implementation.
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

    /// The events currently active on this file.
    ///
    /// If this function returns `POLLIN` or `POLLOUT`, then FileObject will
    /// add `POLLRDNORM` and `POLLWRNORM`, respective, which are equivalent in
    /// the Linux UAPI.
    ///
    /// See https://linux.die.net/man/2/poll
    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        Ok(FdEvents::POLLIN | FdEvents::POLLOUT)
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _request: u32,
        _arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        error!(ENOTTY)
    }

    fn fcntl(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        cmd: u32,
        _arg: u64,
    ) -> Result<SyscallResult, Errno> {
        default_fcntl(cmd)
    }

    /// Return a handle that allows access to this file descritor through the zxio protocols.
    ///
    /// If None is returned, the file will act as if it was a fd to `/dev/null`.
    fn to_handle(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
    ) -> Result<Option<zx::NullableHandle>, Errno> {
        serve_file(current_task, file, current_task.current_creds().clone())
            .map(|c| Some(c.0.into_channel().into()))
    }

    // Return a vector of handles. This is used in situations where there is more than one handle
    // associated with this file descriptor.
    //
    // In Fuchsia, there is an expectation that there is a 1:1 mapping between a file descriptor and
    // a handle. In general, we do not want to violate that rule. This function is intended to used
    // in very limited circumstances (compatibility with Linux and Binder), where we need to violate
    // rule.
    //
    // Specifically, we are using this to implement SyncFiles correctly, where a single SyncFile can
    // represent multiple SyncPoints. Each SyncPoint contains a zx::Counter.
    //
    // If you chose to implement this function, to_handle() should return an error. You must also be
    // aware that if these handles are passed to Fuchsia over Binder, they will be represented as
    // single file descriptor, and you should use the composite_fd library to manage that file
    // descriptor.
    fn get_handles(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<Vec<zx::NullableHandle>, Errno> {
        error!(ENOTSUP)
    }

    /// Returns the associated Pid.
    ///
    /// Used by pidfd and `/proc/<pid>`. Unlikely to be used by other files.
    fn as_pid(&self, _file: &FileObject) -> Result<Pid, Errno> {
        error!(EBADF)
    }

    fn readahead(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _length: usize,
    ) -> Result<(), Errno> {
        error!(EINVAL)
    }

    /// Extra information that is included in the /proc/<pid>/fdfino/<fd> entry.
    fn extra_fdinfo(&self, _file: &FileHandle, _current_task: &CurrentTask) -> Option<FsString> {
        None
    }
}

/// Marker trait for implementation of FileOps that do not need to implement `close` and can
/// then pass a wrapper object as the `FileOps` implementation.
pub trait CloseFreeSafe {}
impl<T: FileOps + CloseFreeSafe, P: Deref<Target = T> + Send + Sync + 'static> FileOps for P {
    fn close(self: Box<Self>, _file: &FileObjectState, _current_task: &CurrentTask) {
        // This method cannot be delegated. T being `CloseFreeSafe` this is fine.
    }

    fn flush(&self, file: &FileObject, current_task: &CurrentTask) {
        self.deref().flush(file, current_task)
    }

    fn has_persistent_offsets(&self) -> bool {
        self.deref().has_persistent_offsets()
    }

    fn writes_update_seek_offset(&self) -> bool {
        self.deref().writes_update_seek_offset()
    }

    fn is_seekable(&self) -> bool {
        self.deref().is_seekable()
    }

    fn read(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        self.deref().read(file, current_task, offset, data)
    }

    fn write(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        self.deref().write(file, current_task, offset, data)
    }

    fn seek(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        current_offset: off_t,
        target: SeekTarget,
    ) -> Result<off_t, Errno> {
        self.deref().seek(file, current_task, current_offset, target)
    }

    fn sync(&self, file: &FileObject, current_task: &CurrentTask) -> Result<(), Errno> {
        self.deref().sync(file, current_task)
    }

    fn data_sync(&self, file: &FileObject, current_task: &CurrentTask) -> Result<(), Errno> {
        self.deref().data_sync(file, current_task)
    }

    fn get_memory(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        length: Option<usize>,
        prot: ProtectionFlags,
    ) -> Result<Arc<MemoryObject>, Errno> {
        self.deref().get_memory(file, current_task, length, prot)
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
        self.deref().mmap(file, current_task, addr, memory_offset, length, prot_flags, options)
    }

    fn readdir(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        sink: &mut dyn DirentSink,
    ) -> Result<(), Errno> {
        self.deref().readdir(file, current_task, sink)
    }

    fn wait_async(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        self.deref().wait_async(file, current_task, waiter, events, handler)
    }

    fn query_events(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        self.deref().query_events(file, current_task)
    }

    fn ioctl(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        self.deref().ioctl(file, current_task, request, arg)
    }

    fn fcntl(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        cmd: u32,
        arg: u64,
    ) -> Result<SyscallResult, Errno> {
        self.deref().fcntl(file, current_task, cmd, arg)
    }

    fn to_handle(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
    ) -> Result<Option<zx::NullableHandle>, Errno> {
        self.deref().to_handle(file, current_task)
    }

    fn get_handles(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
    ) -> Result<Vec<zx::NullableHandle>, Errno> {
        self.deref().get_handles(file, current_task)
    }

    fn as_pid(&self, file: &FileObject) -> Result<Pid, Errno> {
        self.deref().as_pid(file)
    }

    fn readahead(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        length: usize,
    ) -> Result<(), Errno> {
        self.deref().readahead(file, current_task, offset, length)
    }

    fn extra_fdinfo(&self, file: &FileHandle, current_task: &CurrentTask) -> Option<FsString> {
        self.deref().extra_fdinfo(file, current_task)
    }
}

pub fn default_eof_offset(file: &FileObject, current_task: &CurrentTask) -> Result<off_t, Errno> {
    Ok(file.node().get_size(current_task)? as off_t)
}

/// Implement the seek method for a file. The computation from the end of the file must be provided
/// through a callback.
///
/// Errors if the calculated offset is invalid.
///
/// - `current_offset`: The current position
/// - `target`: The location to seek to.
/// - `compute_end`: Compute the new offset from the end. Return an error if the operation is not
///    supported.
pub fn default_seek<F>(
    current_offset: off_t,
    target: SeekTarget,
    compute_end: F,
) -> Result<off_t, Errno>
where
    F: FnOnce() -> Result<off_t, Errno>,
{
    let new_offset = match target {
        SeekTarget::Set(offset) => Some(offset),
        SeekTarget::Cur(offset) => current_offset.checked_add(offset),
        SeekTarget::End(offset) => compute_end()?.checked_add(offset),
        SeekTarget::Data(offset) => {
            let eof = compute_end().unwrap_or(off_t::MAX);
            if offset >= eof {
                return error!(ENXIO);
            }
            Some(offset)
        }
        SeekTarget::Hole(offset) => {
            let eof = compute_end()?;
            if offset >= eof {
                return error!(ENXIO);
            }
            Some(eof)
        }
    }
    .ok_or_else(|| errno!(EINVAL))?;

    if new_offset < 0 {
        return error!(EINVAL);
    }

    Ok(new_offset)
}

/// Implement the seek method for a file without an upper bound on the resulting offset.
///
/// This is useful for files without a defined size.
///
/// Errors if the calculated offset is invalid.
///
/// - `current_offset`: The current position
/// - `target`: The location to seek to.
pub fn unbounded_seek(current_offset: off_t, target: SeekTarget) -> Result<off_t, Errno> {
    default_seek(current_offset, target, || Ok(MAX_LFS_FILESIZE as off_t))
}

#[macro_export]
macro_rules! fileops_impl_delegate_read_write_and_seek {
    ($self:ident, $delegate:expr) => {
        fn is_seekable(&self) -> bool {
            true
        }

        fn read(
            &$self,
            file: &FileObject,
            current_task: &$crate::task::CurrentTask,
            offset: usize,
            data: &mut dyn $crate::vfs::buffers::OutputBuffer,
        ) -> Result<usize, starnix_uapi::errors::Errno> {
            $delegate.read(file, current_task, offset, data)
        }

        fn write(
            &$self,
            file: &FileObject,
            current_task: &$crate::task::CurrentTask,
            offset: usize,
            data: &mut dyn $crate::vfs::buffers::InputBuffer,
        ) -> Result<usize, starnix_uapi::errors::Errno> {
            $delegate.write(file, current_task, offset, data)
        }

        fn seek(
            &$self,
            file: &FileObject,
            current_task: &$crate::task::CurrentTask,
            current_offset: starnix_uapi::off_t,
            target: $crate::vfs::SeekTarget,
        ) -> Result<starnix_uapi::off_t, starnix_uapi::errors::Errno> {
            $delegate.seek(file, current_task, current_offset, target)
        }
    };
}

/// Implements [`FileOps::seek`] in a way that makes sense for seekable files.
#[macro_export]
macro_rules! fileops_impl_seekable {
    () => {
        fn is_seekable(&self) -> bool {
            true
        }

        fn seek(
            &self,
            file: &$crate::vfs::FileObject,
            current_task: &$crate::task::CurrentTask,
            current_offset: starnix_uapi::off_t,
            target: $crate::vfs::SeekTarget,
        ) -> Result<starnix_uapi::off_t, starnix_uapi::errors::Errno> {
            $crate::vfs::default_seek(current_offset, target, || {
                $crate::vfs::default_eof_offset(file, current_task)
            })
        }
    };
}

/// Implements [`FileOps`] methods in a way that makes sense for non-seekable files.
#[macro_export]
macro_rules! fileops_impl_nonseekable {
    () => {
        fn is_seekable(&self) -> bool {
            false
        }

        fn seek(
            &self,
            _file: &$crate::vfs::FileObject,
            _current_task: &$crate::task::CurrentTask,
            _current_offset: starnix_uapi::off_t,
            _target: $crate::vfs::SeekTarget,
        ) -> Result<starnix_uapi::off_t, starnix_uapi::errors::Errno> {
            starnix_uapi::error!(ESPIPE)
        }
    };
}

/// Implements [`FileOps::seek`] methods in a way that makes sense for files that ignore
/// seeking operations and always read/write at offset 0.
#[macro_export]
macro_rules! fileops_impl_seekless {
    () => {
        fn has_persistent_offsets(&self) -> bool {
            false
        }

        fn is_seekable(&self) -> bool {
            true
        }

        fn seek(
            &self,
            _file: &$crate::vfs::FileObject,
            _current_task: &$crate::task::CurrentTask,
            _current_offset: starnix_uapi::off_t,
            _target: $crate::vfs::SeekTarget,
        ) -> Result<starnix_uapi::off_t, starnix_uapi::errors::Errno> {
            Ok(0)
        }
    };
}

#[macro_export]
macro_rules! fileops_impl_dataless {
    () => {
        fn write(
            &self,
            _file: &$crate::vfs::FileObject,
            _current_task: &$crate::task::CurrentTask,
            _offset: usize,
            _data: &mut dyn $crate::vfs::buffers::InputBuffer,
        ) -> Result<usize, starnix_uapi::errors::Errno> {
            starnix_uapi::error!(EINVAL)
        }

        fn read(
            &self,
            _file: &$crate::vfs::FileObject,
            _current_task: &$crate::task::CurrentTask,
            _offset: usize,
            _data: &mut dyn $crate::vfs::buffers::OutputBuffer,
        ) -> Result<usize, starnix_uapi::errors::Errno> {
            starnix_uapi::error!(EINVAL)
        }
    };
}

/// Implements [`FileOps`] methods in a way that makes sense for directories. You must implement
/// [`FileOps::seek`] and [`FileOps::readdir`].
#[macro_export]
macro_rules! fileops_impl_directory {
    () => {
        fn is_seekable(&self) -> bool {
            true
        }

        fn read(
            &self,
            _file: &$crate::vfs::FileObject,
            _current_task: &$crate::task::CurrentTask,
            _offset: usize,
            _data: &mut dyn $crate::vfs::buffers::OutputBuffer,
        ) -> Result<usize, starnix_uapi::errors::Errno> {
            starnix_uapi::error!(EISDIR)
        }

        fn write(
            &self,
            _file: &$crate::vfs::FileObject,
            _current_task: &$crate::task::CurrentTask,
            _offset: usize,
            _data: &mut dyn $crate::vfs::buffers::InputBuffer,
        ) -> Result<usize, starnix_uapi::errors::Errno> {
            starnix_uapi::error!(EISDIR)
        }
    };
}

#[macro_export]
macro_rules! fileops_impl_unbounded_seek {
    () => {
        fn seek(
            &self,
            _file: &$crate::vfs::FileObject,
            _current_task: &$crate::task::CurrentTask,
            current_offset: starnix_uapi::off_t,
            target: $crate::vfs::SeekTarget,
        ) -> Result<starnix_uapi::off_t, starnix_uapi::errors::Errno> {
            $crate::vfs::unbounded_seek(current_offset, target)
        }
    };
}

#[macro_export]
macro_rules! fileops_impl_noop_sync {
    () => {
        fn sync(
            &self,
            file: &$crate::vfs::FileObject,
            _current_task: &$crate::task::CurrentTask,
        ) -> Result<(), starnix_uapi::errors::Errno> {
            if !file.node().is_reg() && !file.node().is_dir() {
                return starnix_uapi::error!(EINVAL);
            }
            Ok(())
        }
    };
}

// Public re-export of macros allows them to be used like regular rust items.

pub use fileops_impl_dataless;
pub use fileops_impl_delegate_read_write_and_seek;
pub use fileops_impl_directory;
pub use fileops_impl_nonseekable;
pub use fileops_impl_noop_sync;
pub use fileops_impl_seekable;
pub use fileops_impl_seekless;
pub use fileops_impl_unbounded_seek;
pub const AES256_KEY_SIZE: usize = 32;

pub fn canonicalize_ioctl_request(current_task: &CurrentTask, request: u32) -> u32 {
    if current_task.is_arch32() {
        match request {
            uapi::arch32::FS_IOC_GETFLAGS => uapi::FS_IOC_GETFLAGS,
            uapi::arch32::FS_IOC_SETFLAGS => uapi::FS_IOC_SETFLAGS,
            _ => request,
        }
    } else {
        request
    }
}

/// Universal VFS ioctl dispatcher for [`FileObject`].
///
/// Handles generic file system ioctls (such as non-blocking mode toggles, block size queries,
/// and file cloning) at the VFS layer without delegating to underlying device drivers.
///
/// Returns `Some(result)` if the ioctl command is handled by the VFS layer, or `None` if the
/// command is unhandled and should be dispatched to [`FileOps::ioctl`].
pub fn default_vfs_ioctl(
    file: &FileObject,
    current_task: &CurrentTask,
    request: u32,
    arg: SyscallArg,
) -> Result<Option<SyscallResult>, Errno> {
    match canonicalize_ioctl_request(current_task, request) {
        FIGETBSZ if file.node().is_reg() || file.node().is_dir() => {
            let blocksize = file.node().stat(current_task)?.st_blksize;
            current_task.write_object(arg.into(), &blocksize)?;
            Ok(Some(SUCCESS))
        }
        FIONBIO => {
            let arg_ref = UserAddress::from(arg).into();
            let arg: i32 = current_task.read_object(arg_ref)?;
            let val = if arg == 0 {
                // Clear the NONBLOCK flag
                OpenFlags::empty()
            } else {
                // Set the NONBLOCK flag
                OpenFlags::NONBLOCK
            };
            file.update_file_flags(val, OpenFlags::NONBLOCK);
            Ok(Some(SUCCESS))
        }
        FIOQSIZE if file.node().is_reg() || file.node().is_dir() => {
            let size = file.node().stat(current_task)?.st_size;
            current_task.write_object(arg.into(), &size)?;
            Ok(Some(SUCCESS))
        }
        FIONREAD if file.node().is_reg() => {
            track_stub!(TODO("https://fxbug.dev/322874897"), "FIONREAD");
            let size =
                file.node().fetch_and_refresh_info(current_task).map_err(|_| errno!(EINVAL))?.size;
            let offset = usize::try_from(file.offset.read()).map_err(|_| errno!(EINVAL))?;
            let remaining =
                if size < offset { 0 } else { i32::try_from(size - offset).unwrap_or(i32::MAX) };
            current_task.write_object(arg.into(), &remaining)?;
            Ok(Some(SUCCESS))
        }
        FS_IOC_FSGETXATTR => {
            track_stub!(TODO("https://fxbug.dev/322875209"), "FS_IOC_FSGETXATTR");
            let arg = UserAddress::from(arg).into();
            current_task.write_object(arg, &fsxattr::default())?;
            Ok(Some(SUCCESS))
        }
        FS_IOC_FSSETXATTR => {
            track_stub!(TODO("https://fxbug.dev/322875271"), "FS_IOC_FSSETXATTR");
            let arg = UserAddress::from(arg).into();
            let _: fsxattr = current_task.read_object(arg)?;
            Ok(Some(SUCCESS))
        }
        uapi::FS_IOC_GETFLAGS => {
            track_stub!(TODO("https://fxbug.dev/322874935"), "FS_IOC_GETFLAGS");
            let arg = UserRef::<u32>::from(arg);
            let mut flags: u32 = 0;
            if matches!(*file.node().fsverity.lock(), FsVerityState::FsVerity) {
                flags |= FS_VERITY_FL;
            }
            if file.node().info().casefold {
                flags |= FS_CASEFOLD_FL;
            }
            current_task.write_object(arg, &flags)?;
            Ok(Some(SUCCESS))
        }
        uapi::FS_IOC_SETFLAGS => {
            track_stub!(TODO("https://fxbug.dev/322875367"), "FS_IOC_SETFLAGS");
            // In Linux, FS_IOC_SETFLAGS operates on the mount and inode ownership rather than
            // requiring an FD opened for writing (directories can only be opened O_RDONLY) or
            // checking DAC write permissions on the file mode.
            file.name.mount.check_readonly_filesystem()?;
            let arg = UserRef::<u32>::from(arg);
            let flags: u32 = current_task.read_object(arg)?;
            let new_casefold = flags & FS_CASEFOLD_FL != 0;
            file.name.entry.set_casefold(current_task, new_casefold)?;

            Ok(Some(SUCCESS))
        }
        FS_IOC_ENABLE_VERITY => {
            fsverity::ioctl::enable(current_task, UserAddress::from(arg).into(), file).map(Some)
        }
        FS_IOC_MEASURE_VERITY => {
            fsverity::ioctl::measure(current_task, UserAddress::from(arg).into(), file).map(Some)
        }
        FS_IOC_READ_VERITY_METADATA => {
            fsverity::ioctl::read_metadata(current_task, UserAddress::from(arg).into(), file)
                .map(Some)
        }
        FS_IOC_ADD_ENCRYPTION_KEY => {
            let fscrypt_add_key_ref = UserRef::<fscrypt_add_key_arg>::from(arg);
            let key_ref_addr = fscrypt_add_key_ref.next()?.addr();
            let mut fscrypt_add_key_arg = current_task.read_object(fscrypt_add_key_ref.clone())?;
            if fscrypt_add_key_arg.key_id != 0 {
                track_stub!(TODO("https://fxbug.dev/375649227"), "non-zero key ids");
                return error!(ENOTSUP);
            }
            if fscrypt_add_key_arg.key_spec.type_ != FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER {
                track_stub!(TODO("https://fxbug.dev/375648306"), "fscrypt descriptor type");
                return error!(ENOTSUP);
            }
            let key = current_task
                .read_memory_to_vec(key_ref_addr, fscrypt_add_key_arg.raw_size as usize)?;
            let user_id = current_task.current_creds().uid;

            let crypt_service = file.node().fs().crypt_service().ok_or_else(|| errno!(ENOTSUP))?;
            let key_identifier = crypt_service.add_wrapping_key(&key, user_id)?;
            fscrypt_add_key_arg.key_spec.u.identifier =
                fscrypt_identifier { value: key_identifier, ..Default::default() };
            current_task.write_object(fscrypt_add_key_ref, &fscrypt_add_key_arg)?;
            Ok(Some(SUCCESS))
        }
        FS_IOC_SET_ENCRYPTION_POLICY => {
            let fscrypt_policy_ref = UserRef::<uapi::fscrypt_policy_v2>::from(arg);
            let policy = current_task.read_object(fscrypt_policy_ref)?;
            if policy.version as u32 != FSCRYPT_POLICY_V2 {
                track_stub!(TODO("https://fxbug.dev/375649656"), "fscrypt policy v1");
                return error!(ENOTSUP);
            }
            if policy.flags != 0 {
                track_stub!(
                    TODO("https://fxbug.dev/375700939"),
                    "fscrypt policy flags",
                    policy.flags
                );
            }
            if policy.contents_encryption_mode as u32 != FSCRYPT_MODE_AES_256_XTS {
                track_stub!(
                    TODO("https://fxbug.dev/375684057"),
                    "fscrypt encryption modes",
                    policy.contents_encryption_mode
                );
            }
            if policy.filenames_encryption_mode as u32 != FSCRYPT_MODE_AES_256_CTS {
                track_stub!(
                    TODO("https://fxbug.dev/375684057"),
                    "fscrypt encryption modes",
                    policy.filenames_encryption_mode
                );
            }
            let user_id = current_task.current_creds().uid;
            if user_id != file.node().info().uid {
                security::check_task_capable(current_task, CAP_FOWNER)
                    .map_err(|_| errno!(EACCES))?;
            }

            let crypt_service = file.node().fs().crypt_service().ok_or_else(|| errno!(ENOTSUP))?;
            if let Some(users) =
                crypt_service.get_users_for_key(EncryptionKeyId::from(policy.master_key_identifier))
            {
                if !users.contains(&user_id) {
                    return error!(ENOKEY);
                }
            } else {
                track_stub!(
                    TODO("https://fxbug.dev/375067633"),
                    "users with CAP_FOWNER can set encryption policies with unadded keys"
                );
                return error!(ENOKEY);
            }

            let attributes = file.node().fetch_and_refresh_info(current_task)?;
            if let Some(wrapping_key_id) = &attributes.wrapping_key_id {
                if wrapping_key_id != &policy.master_key_identifier {
                    return error!(EEXIST);
                }
            } else {
                // Don't deadlock! update_attributes will also lock the attributes.
                std::mem::drop(attributes);
                file.node().update_attributes(current_task, |info| {
                    info.wrapping_key_id = Some(policy.master_key_identifier);
                    Ok(())
                })?;
            }
            Ok(Some(SUCCESS))
        }
        FS_IOC_REMOVE_ENCRYPTION_KEY => {
            let fscrypt_remove_key_arg_ref = UserRef::<uapi::fscrypt_remove_key_arg>::from(arg);
            let fscrypt_remove_key_arg = current_task.read_object(fscrypt_remove_key_arg_ref)?;
            if fscrypt_remove_key_arg.key_spec.type_ != FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER {
                track_stub!(TODO("https://fxbug.dev/375648306"), "fscrypt descriptor type");
                return error!(ENOTSUP);
            }
            let crypt_service = file.node().fs().crypt_service().ok_or_else(|| errno!(ENOTSUP))?;
            let user_id = current_task.current_creds().uid;
            #[allow(
                clippy::undocumented_unsafe_blocks,
                reason = "Force documented unsafe blocks in Starnix"
            )]
            let identifier = unsafe { fscrypt_remove_key_arg.key_spec.u.identifier.value };
            crypt_service.forget_wrapping_key(identifier, user_id)?;
            Ok(Some(SUCCESS))
        }
        linux_uapi::FICLONE | linux_uapi::FICLONERANGE | linux_uapi::FIDEDUPERANGE => {
            error!(EOPNOTSUPP)
        }
        _ => Ok(None),
    }
}

pub fn default_fcntl(cmd: u32) -> Result<SyscallResult, Errno> {
    track_stub!(TODO("https://fxbug.dev/322875704"), "default fcntl", cmd);
    error!(EINVAL)
}

pub fn default_mmap(
    file: &FileObject,
    current_task: &CurrentTask,
    addr: DesiredAddress,
    memory_offset: u64,
    length: usize,
    prot_flags: ProtectionFlags,
    options: MappingOptions,
) -> Result<UserAddress, Errno> {
    fuchsia_trace::duration!(CATEGORY_STARNIX_MM, "FileOpsDefaultMmap");
    let min_memory_size = (memory_offset as usize)
        .checked_add(round_up_to_system_page_size(length)?)
        .ok_or_else(|| errno!(EINVAL))?;
    let mut memory = if options.contains(MappingOptions::SHARED) {
        fuchsia_trace::duration!(CATEGORY_STARNIX_MM, "GetSharedVmo");
        file.ops.get_memory(file, current_task, Some(min_memory_size), prot_flags)?
    } else {
        fuchsia_trace::duration!(CATEGORY_STARNIX_MM, "GetPrivateVmo");
        // TODO(tbodt): Use PRIVATE_CLONE to have the filesystem server do the clone for us.
        let base_prot_flags = (prot_flags | ProtectionFlags::READ) - ProtectionFlags::WRITE;
        let memory =
            file.ops.get_memory(file, current_task, Some(min_memory_size), base_prot_flags)?;
        let mut clone_flags = zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE;
        if !prot_flags.contains(ProtectionFlags::WRITE) {
            clone_flags |= zx::VmoChildOptions::NO_WRITE;
        }
        fuchsia_trace::duration!(CATEGORY_STARNIX_MM, "CreatePrivateChildVmo");
        Arc::new(memory.create_child(clone_flags, 0, memory.get_size()).map_err(impossible_error)?)
    };

    // Write guard is necessary only for shared mappings. Note that this doesn't depend on
    // `prot_flags` since these can be changed later with `mprotect()`.
    let file_write_guard = if options.contains(MappingOptions::SHARED) && file.can_write() {
        let node = &file.name.entry.node;
        let state = node.write_guard_state.lock();

        // `F_SEAL_FUTURE_WRITE` should allow `mmap(PROT_READ)`, but block
        // `mprotect(PROT_WRITE)`. This is different from `F_SEAL_WRITE`, which blocks
        // `mmap(PROT_READ)`. To handle this case correctly remove `WRITE` right from the
        // VMO handle to ensure `mprotect(PROT_WRITE)` fails.
        let seals = state.get_seals().unwrap_or(SealFlags::empty());
        if seals.contains(SealFlags::FUTURE_WRITE)
            && !seals.contains(SealFlags::WRITE)
            && !prot_flags.contains(ProtectionFlags::WRITE)
        {
            let mut new_rights = zx::Rights::VMO_DEFAULT - zx::Rights::WRITE;
            if prot_flags.contains(ProtectionFlags::EXEC) {
                new_rights |= zx::Rights::EXECUTE;
            }
            memory = Arc::new(memory.duplicate_handle(new_rights).map_err(impossible_error)?);

            None
        } else {
            Some(FileWriteGuardMode::WriteMapping)
        }
    } else {
        None
    };

    current_task.mm()?.map_memory(
        addr,
        memory,
        memory_offset,
        length,
        prot_flags,
        options,
        MappingName::File(file.to_mapping(file_write_guard)?),
    )
}

pub struct OPathOps {}

impl OPathOps {
    pub fn new() -> OPathOps {
        OPathOps {}
    }
}

impl FileOps for OPathOps {
    fileops_impl_noop_sync!();

    fn has_persistent_offsets(&self) -> bool {
        false
    }
    fn is_seekable(&self) -> bool {
        true
    }
    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        error!(EBADF)
    }
    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        error!(EBADF)
    }
    fn seek(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _current_offset: off_t,
        _target: SeekTarget,
    ) -> Result<off_t, Errno> {
        error!(EBADF)
    }
    fn get_memory(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _length: Option<usize>,
        _prot: ProtectionFlags,
    ) -> Result<Arc<MemoryObject>, Errno> {
        error!(EBADF)
    }
    fn readdir(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _sink: &mut dyn DirentSink,
    ) -> Result<(), Errno> {
        error!(EBADF)
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _request: u32,
        _arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        error!(EBADF)
    }
}

pub struct ProxyFileOps(pub FileHandle);

impl FileOps for ProxyFileOps {
    // `close` is not delegated because the last reference to a `ProxyFileOps` is not
    // necessarily the last reference of the proxied file. If this is the case, the
    // releaser will handle it.
    // These don't take &FileObject making it too hard to handle them properly in the macro
    fn has_persistent_offsets(&self) -> bool {
        self.0.ops().has_persistent_offsets()
    }
    fn writes_update_seek_offset(&self) -> bool {
        self.0.ops().writes_update_seek_offset()
    }
    fn is_seekable(&self) -> bool {
        self.0.ops().is_seekable()
    }
    fn flush(&self, _file: &FileObject, current_task: &CurrentTask) {
        self.0.ops().flush(&self.0, current_task);
    }
    fn wait_async(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        self.0.ops().wait_async(&self.0, current_task, waiter, events, handler)
    }
    fn query_events(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        self.0.ops().query_events(&self.0, current_task)
    }
    fn read(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        self.0.ops().read(&self.0, current_task, offset, data)
    }
    fn write(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        self.0.ops().write(&self.0, current_task, offset, data)
    }
    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        self.0.ops().ioctl(&self.0, current_task, request, arg)
    }
    fn fcntl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        cmd: u32,
        arg: u64,
    ) -> Result<SyscallResult, Errno> {
        self.0.ops().fcntl(&self.0, current_task, cmd, arg)
    }
    fn readdir(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        sink: &mut dyn DirentSink,
    ) -> Result<(), Errno> {
        self.0.ops().readdir(&self.0, current_task, sink)
    }
    fn sync(&self, _file: &FileObject, current_task: &CurrentTask) -> Result<(), Errno> {
        self.0.ops().sync(&self.0, current_task)
    }
    fn data_sync(&self, _file: &FileObject, current_task: &CurrentTask) -> Result<(), Errno> {
        self.0.ops().sync(&self.0, current_task)
    }
    fn get_memory(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        length: Option<usize>,
        prot: ProtectionFlags,
    ) -> Result<Arc<MemoryObject>, Errno> {
        self.0.ops.get_memory(&self.0, current_task, length, prot)
    }
    fn mmap(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        addr: DesiredAddress,
        memory_offset: u64,
        length: usize,
        prot_flags: ProtectionFlags,
        options: MappingOptions,
    ) -> Result<UserAddress, Errno> {
        self.0.ops.mmap(&self.0, current_task, addr, memory_offset, length, prot_flags, options)
    }
    fn seek(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        offset: off_t,
        target: SeekTarget,
    ) -> Result<off_t, Errno> {
        self.0.ops.seek(&self.0, current_task, offset, target)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum FileAsyncOwner {
    #[default]
    Unowned,
    Thread(Option<Pid>),
    Process(Option<Pid>),
    ProcessGroup(Option<Pid>),
}

impl FileAsyncOwner {
    pub fn new(current_task: &CurrentTask, pid: pid_t) -> Result<Self, Errno> {
        if pid == 0 {
            Ok(Self::Unowned)
        } else if pid > 0 {
            let entry = current_task.kernel().pids.get(pid)?;
            entry.get_task()?;
            Ok(Self::Process(Some(entry)))
        } else {
            let pgid = pid.checked_neg().ok_or_else(|| errno!(EINVAL))?;
            let entry = current_task.kernel().pids.get(pgid)?;
            entry.get_process_group()?;
            Ok(Self::ProcessGroup(Some(entry)))
        }
    }

    pub fn new_ex(
        current_task: &CurrentTask,
        requested_owner: uapi::f_owner_ex,
    ) -> Result<Self, Errno> {
        let pids = &current_task.kernel().pids;
        let get_pid = |lookup_pg: bool| -> Result<Option<Pid>, Errno> {
            if requested_owner.pid == 0 {
                Ok(None)
            } else {
                let entry = pids.get(requested_owner.pid)?;
                if lookup_pg {
                    entry.get_process_group()?;
                } else {
                    entry.get_task()?;
                }
                Ok(Some(entry))
            }
        };
        match requested_owner.type_ as u32 {
            F_OWNER_TID => Ok(Self::Thread(get_pid(false)?)),
            F_OWNER_PID => Ok(Self::Process(get_pid(false)?)),
            F_OWNER_PGRP => Ok(Self::ProcessGroup(get_pid(true)?)),
            _ => error!(EINVAL),
        }
    }

    pub fn get_owner(&self) -> pid_t {
        match self {
            Self::Unowned | Self::Thread(None) | Self::Process(None) | Self::ProcessGroup(None) => {
                0
            }
            Self::Thread(Some(pid)) | Self::Process(Some(pid)) => pid.id,
            Self::ProcessGroup(Some(pid)) => -pid.id,
        }
    }

    pub fn get_owner_ex(&self) -> uapi::f_owner_ex {
        match self {
            Self::Unowned => uapi::f_owner_ex { type_: F_OWNER_TID as i32, pid: 0 },
            Self::Thread(pid) => uapi::f_owner_ex {
                type_: F_OWNER_TID as i32,
                pid: pid.as_ref().map_or(0, |p| p.id),
            },
            Self::Process(pid) => uapi::f_owner_ex {
                type_: F_OWNER_PID as i32,
                pid: pid.as_ref().map_or(0, |p| p.id),
            },
            Self::ProcessGroup(pid) => uapi::f_owner_ex {
                type_: F_OWNER_PGRP as i32,
                pid: pid.as_ref().map_or(0, |p| p.id),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileObjectId(u64);

impl FileObjectId {
    pub fn as_epoll_key(&self) -> EpollKey {
        self.0 as EpollKey
    }
}

/// A session with a file object.
///
/// Each time a client calls open(), we create a new FileObject from the
/// underlying FsNode that receives the open(). This object contains the state
/// that is specific to this sessions whereas the underlying FsNode contains
/// the state that is shared between all the sessions.
pub struct FileObject {
    ops: Box<dyn FileOps>,
    state: FileObjectState,
}

impl std::ops::Deref for FileObject {
    type Target = FileObjectState;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

pub struct FileObjectState {
    /// Weak reference to the `FileHandle` of this `FileObject`. This allows to retrieve the
    /// `FileHandle` from a `FileObject`.
    pub weak_handle: WeakFileHandle,

    /// A unique identifier for this file object.
    pub id: FileObjectId,

    /// The NamespaceNode associated with this FileObject.
    ///
    /// Represents the name the process used to open this file.
    pub name: ActiveNamespaceNode,

    pub fs: FileSystemHandle,

    pub offset: RcuAtomic<off_t, FileObjectOffset>,

    flags: AtomicOpenFlags,

    async_owner: LockDepMutex<FileAsyncOwner, FileAsyncOwnerLock>,

    /// A set of epoll file descriptor numbers that tracks which `EpollFileObject`s add this
    /// `FileObject` as the control file.
    epoll_files: LockDepMutex<HashMap<FileHandleKey, WeakFileHandle>, FileEpollFilesLock>,

    /// See fcntl F_SETLEASE and F_GETLEASE.
    lease: LockDepMutex<FileLeaseType, FileLeaseLock>,

    // This extra reference to the FsNode should not be needed, but it is needed to make
    // Inotify.ExcludeUnlinkInodeEvents pass.
    _mysterious_node: Option<FsNodeHandle>,

    /// Opaque security state associated this file object.
    pub security_state: security::FileObjectState,
}

pub enum FileObjectReleaserAction {}
impl ReleaserAction<FileObject> for FileObjectReleaserAction {
    fn release(file_object: ReleaseGuard<FileObject>) {
        register_delayed_release(file_object);
    }
}
pub type FileReleaser = ObjectReleaser<FileObject, FileObjectReleaserAction>;
pub type FileHandle = Arc<FileReleaser>;
pub type WeakFileHandle = Weak<FileReleaser>;
pub type FileHandleKey = WeakKey<FileReleaser>;

impl FileObjectState {
    /// The FsNode from which this FileObject was created.
    pub fn node(&self) -> &FsNodeHandle {
        &self.name.entry.node
    }

    pub fn flags(&self) -> OpenFlags {
        self.flags.load(Ordering::Relaxed)
    }

    pub fn can_read(&self) -> bool {
        self.flags.load(Ordering::Relaxed).can_read()
    }

    pub fn can_write(&self) -> bool {
        self.flags.load(Ordering::Relaxed).can_write()
    }

    /// Returns false if the file is not allowed to be executed.
    pub fn can_exec(&self) -> bool {
        let mounted_no_exec = self.name.to_passive().mount.flags().contains(MountFlags::NOEXEC);
        let no_exec_seal = self
            .node()
            .write_guard_state
            .lock()
            .get_seals()
            .map(|seals| seals.contains(SealFlags::NO_EXEC))
            .unwrap_or(false);
        !(mounted_no_exec || no_exec_seal)
    }

    // Notifies watchers on the current node and its parent about an event.
    pub fn notify(&self, event_mask: InotifyMask) {
        self.name.notify(event_mask)
    }
}

impl FileObject {
    /// Create a FileObject that is not mounted in a namespace.
    ///
    /// In particular, this will create a new unrooted entries. This should not be used on
    /// file system with persistent entries, as the created entry will be out of sync with the one
    /// from the file system.
    ///
    /// The returned FileObject does not have a name.
    pub fn new_anonymous(
        current_task: &CurrentTask,
        ops: Box<dyn FileOps>,
        node: FsNodeHandle,
        flags: OpenFlags,
    ) -> FileHandle {
        assert!(!node.fs().has_permanent_entries());
        Self::new(
            current_task,
            ops,
            NamespaceNode::new_anonymous_unrooted(current_task, node),
            flags,
        )
        .expect("Failed to create anonymous FileObject")
    }

    /// Create a FileObject with an associated NamespaceNode.
    ///
    /// This function is not typically called directly. Instead, consider
    /// calling NamespaceNode::open.
    pub fn new(
        current_task: &CurrentTask,
        ops: Box<dyn FileOps>,
        name: NamespaceNode,
        flags: OpenFlags,
    ) -> Result<FileHandle, Errno> {
        let _mysterious_node = if flags.can_write() {
            name.entry.node.write_guard_state.lock().acquire(FileWriteGuardMode::WriteFile)?;
            Some(name.entry.node.clone())
        } else {
            None
        };
        let fs = name.entry.node.fs();
        let id = FileObjectId(current_task.kernel.next_file_object_id.next());
        let security_state = security::file_alloc_security(current_task);
        let file = FileHandle::new_cyclic(|weak_handle| {
            Self {
                ops,
                state: FileObjectState {
                    weak_handle: weak_handle.clone(),
                    id,
                    name: name.into_active(),
                    fs,
                    offset: RcuAtomic::new(0),
                    flags: AtomicOpenFlags::new(flags - OpenFlags::CREAT),
                    async_owner: Default::default(),
                    epoll_files: Default::default(),
                    lease: Default::default(),
                    _mysterious_node,
                    security_state,
                },
            }
            .into()
        });
        file.notify(InotifyMask::OPEN);

        file.ops().open(&file, current_task)?;
        Ok(file)
    }

    pub fn max_access_for_memory_mapping(&self) -> Access {
        let mut access = Access::EXIST;
        if self.can_exec() {
            access |= Access::EXEC;
        }
        let flags = self.flags.load(Ordering::Relaxed);
        if flags.can_read() {
            access |= Access::READ;
        }
        if flags.can_write() {
            access |= Access::WRITE;
        }
        access
    }

    pub fn ops(&self) -> &dyn FileOps {
        self.ops.as_ref()
    }

    pub fn ops_type_name(&self) -> &'static str {
        self.ops().type_name()
    }

    pub fn is_non_blocking(&self) -> bool {
        self.flags().contains(OpenFlags::NONBLOCK)
    }

    /// Common implementation for blocking operations.
    ///
    /// This function is used to implement the blocking operations for file objects. FileOps
    /// implementations should call this function to handle the blocking logic.
    ///
    /// The `op` parameter is a function that implements the non-blocking version of the operation.
    /// The function is called once without registering a waiter in case no wait is needed. If the
    /// operation returns EAGAIN and the file object is non-blocking, the function returns EAGAIN.
    ///
    /// If the operation returns EAGAIN and the file object is blocking, the function will block
    /// until the given events are triggered. At that time, the operation is retried. Notice that
    /// the `op` function can be called multiple times before the operation completes.
    ///
    /// The `deadline` parameter is the deadline for the operation. If the operation does not
    /// complete before the deadline, the function will return ETIMEDOUT.
    pub fn blocking_op<T, Op>(
        &self,
        current_task: &CurrentTask,
        events: FdEvents,
        deadline: Option<zx::MonotonicInstant>,
        mut op: Op,
    ) -> Result<T, Errno>
    where
        Op: FnMut() -> Result<T, Errno>,
    {
        // Don't return EAGAIN for directories. This can happen because glibc always opens a
        // directory with O_NONBLOCK.
        let can_return_eagain = self.flags().contains(OpenFlags::NONBLOCK)
            && !self.flags().contains(OpenFlags::DIRECTORY);
        // Run the operation a first time without registering a waiter in case no wait is needed.
        match op() {
            Err(errno) if errno == EAGAIN && !can_return_eagain => {}
            result => return result,
        }

        let waiter = Waiter::new();
        loop {
            // Register the waiter before running the operation to prevent a race.
            self.wait_async(current_task, &waiter, events, WaitCallback::none());
            match op() {
                Err(e) if e == EAGAIN => {}
                result => return result,
            }
            waiter
                .wait_until(current_task, deadline.unwrap_or(zx::MonotonicInstant::INFINITE))
                .map_err(|e| if e == ETIMEDOUT { errno!(EAGAIN) } else { e })?;
        }
    }

    pub fn is_seekable(&self) -> bool {
        self.ops().is_seekable()
    }

    pub fn has_persistent_offsets(&self) -> bool {
        self.ops().has_persistent_offsets()
    }

    /// Common implementation for `read` and `read_at`.
    fn read_internal<R>(&self, current_task: &CurrentTask, read: R) -> Result<usize, Errno>
    where
        R: FnOnce() -> Result<usize, Errno>,
    {
        security::file_permission(current_task, self, security::PermissionFlags::READ)?;

        if !self.can_read() {
            return error!(EBADF);
        }
        let bytes_read = read()?;

        // TODO(steveaustin) - omit updating time_access to allow info to be immutable
        // and thus allow simultaneous reads.
        self.update_atime();
        if bytes_read > 0 {
            self.notify(InotifyMask::ACCESS);
        }

        Ok(bytes_read)
    }

    pub fn read(
        &self,
        current_task: &CurrentTask,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        self.read_internal(current_task, || {
            if !self.ops().has_persistent_offsets() {
                if data.available() > MAX_LFS_FILESIZE {
                    return error!(EINVAL);
                }
                return self.ops.read(self, current_task, 0, data);
            }

            let mut offset_guard = self.offset.copy();
            let offset = *offset_guard as usize;
            checked_add_offset_and_length(offset, data.available())?;
            let read = self.ops.read(self, current_task, offset, data)?;
            *offset_guard += read as off_t;
            offset_guard.update();
            Ok(read)
        })
    }

    pub fn read_at(
        &self,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        if !self.ops().is_seekable() {
            return error!(ESPIPE);
        }
        checked_add_offset_and_length(offset, data.available())?;
        self.read_internal(current_task, || self.ops.read(self, current_task, offset, data))
    }

    /// Common checks before calling ops().write.
    fn write_common(
        &self,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        security::file_permission(current_task, self, security::PermissionFlags::WRITE)?;

        // We need to cap the size of `data` to prevent us from growing the file too large,
        // according to <https://man7.org/linux/man-pages/man2/write.2.html>:
        //
        //   The number of bytes written may be less than count if, for example, there is
        //   insufficient space on the underlying physical medium, or the RLIMIT_FSIZE resource
        //   limit is encountered (see setrlimit(2)),
        checked_add_offset_and_length(offset, data.available())?;
        self.ops().write(self, current_task, offset, data)
    }

    /// Common wrapper work for `write` and `write_at`.
    fn write_fn<W>(&self, current_task: &CurrentTask, write: W) -> Result<usize, Errno>
    where
        W: FnOnce() -> Result<usize, Errno>,
    {
        if !self.can_write() {
            return error!(EBADF);
        }
        self.node().clear_suid_and_sgid_bits(current_task)?;
        let bytes_written = write()?;
        self.node().update_ctime_mtime();

        if bytes_written > 0 {
            self.notify(InotifyMask::MODIFY);
        }

        Ok(bytes_written)
    }

    pub fn write(
        &self,
        current_task: &CurrentTask,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        self.write_fn(current_task, || {
            if !self.ops().has_persistent_offsets() {
                return self.write_common(current_task, 0, data);
            }
            let mut offset = self.offset.copy();
            let bytes_written = if self.flags().contains(OpenFlags::APPEND) {
                let _guard = self.node().ops().append_lock_write(self.node(), current_task)?;
                *offset = self.ops().seek(self, current_task, *offset, SeekTarget::End(0))?;
                self.write_common(current_task, *offset as usize, data)
            } else {
                let _guard = self.node().ops().append_lock_read(self.node(), current_task)?;
                self.write_common(current_task, *offset as usize, data)
            }?;
            if self.ops().writes_update_seek_offset() {
                *offset += bytes_written as off_t;
            }
            offset.update();
            Ok(bytes_written)
        })
    }

    pub fn write_at(
        &self,
        current_task: &CurrentTask,
        mut offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        if !self.ops().is_seekable() {
            return error!(ESPIPE);
        }
        self.write_fn(current_task, || {
            if self.flags().contains(OpenFlags::APPEND) {
                let _guard = self.node().append_lock.write(current_task)?;
                // According to LTP test pwrite04:
                //
                //   POSIX requires that opening a file with the O_APPEND flag should have no effect on the
                //   location at which pwrite() writes data. However, on Linux, if a file is opened with
                //   O_APPEND, pwrite() appends data to the end of the file, regardless of the value of offset.
                if self.ops().is_seekable() {
                    checked_add_offset_and_length(offset, data.available())?;
                    offset = default_eof_offset(self, current_task)? as usize;
                }
                self.write_common(current_task, offset, data)
            } else {
                let _guard = self.node().append_lock.read(current_task)?;
                self.write_common(current_task, offset, data)
            }
        })
    }

    pub fn seek(&self, current_task: &CurrentTask, target: SeekTarget) -> Result<off_t, Errno> {
        if !self.ops().is_seekable() {
            return error!(ESPIPE);
        }

        if !self.ops().has_persistent_offsets() {
            return self.ops().seek(self, current_task, 0, target);
        }

        let mut offset_guard = self.offset.copy();
        let new_offset = self.ops().seek(self, current_task, *offset_guard, target)?;
        *offset_guard = new_offset;
        offset_guard.update();
        Ok(new_offset)
    }

    pub fn sync(&self, current_task: &CurrentTask) -> Result<(), Errno> {
        self.ops().sync(self, current_task)
    }

    pub fn data_sync(&self, current_task: &CurrentTask) -> Result<(), Errno> {
        self.ops().data_sync(self, current_task)
    }

    pub fn get_memory(
        &self,
        current_task: &CurrentTask,
        length: Option<usize>,
        prot: ProtectionFlags,
    ) -> Result<Arc<MemoryObject>, Errno> {
        if prot.contains(ProtectionFlags::READ) && !self.can_read() {
            return error!(EACCES);
        }
        if prot.contains(ProtectionFlags::WRITE) && !self.can_write() {
            return error!(EACCES);
        }
        if prot.contains(ProtectionFlags::EXEC) && !self.can_exec() {
            return error!(EPERM);
        }
        self.ops().get_memory(self, current_task, length, prot)
    }

    pub fn mmap(
        &self,
        current_task: &CurrentTask,
        addr: DesiredAddress,
        memory_offset: u64,
        length: usize,
        prot_flags: ProtectionFlags,
        options: MappingOptions,
    ) -> Result<UserAddress, Errno> {
        if !self.can_read() {
            return error!(EACCES);
        }
        if prot_flags.contains(ProtectionFlags::WRITE)
            && !self.can_write()
            && options.contains(MappingOptions::SHARED)
        {
            return error!(EACCES);
        }
        if prot_flags.contains(ProtectionFlags::EXEC) && !self.can_exec() {
            return error!(EPERM);
        }
        self.ops().mmap(self, current_task, addr, memory_offset, length, prot_flags, options)
    }

    pub fn readdir(
        &self,
        current_task: &CurrentTask,
        sink: &mut dyn DirentSink,
    ) -> Result<(), Errno> {
        if self.name.entry.is_dead() {
            return error!(ENOENT);
        }

        security::file_permission(current_task, self, security::PermissionFlags::READ)?;

        self.ops().readdir(self, current_task, sink)?;
        self.update_atime();
        self.notify(InotifyMask::ACCESS);
        Ok(())
    }

    pub fn ioctl(
        &self,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        security::check_file_ioctl_access(current_task, &self, request)?;

        if request == FIBMAP {
            security::check_task_capable(current_task, CAP_SYS_RAWIO)?;

            // TODO: https://fxbug.dev/404795644 - eliminate this phoney response when the SELinux
            // Test Suite no longer requires it.
            if current_task.kernel().features.selinux_test_suite {
                let phoney_block = 0xbadf000du32;
                current_task.write_object(arg.into(), &phoney_block)?;
                return Ok(SUCCESS);
            }
        }

        if let Some(result) = default_vfs_ioctl(self, current_task, request, arg)? {
            return Ok(result);
        }

        self.ops().ioctl(self, current_task, request, arg)
    }

    pub fn fcntl(
        &self,
        current_task: &CurrentTask,
        cmd: u32,
        arg: u64,
    ) -> Result<SyscallResult, Errno> {
        self.ops().fcntl(self, current_task, cmd, arg)
    }

    pub fn ftruncate(&self, current_task: &CurrentTask, length: u64) -> Result<(), Errno> {
        // The file must be opened with write permissions. Otherwise
        // truncating it is forbidden.
        if !self.can_write() {
            return error!(EINVAL);
        }
        self.node().ftruncate(current_task, length)?;
        self.name.entry.notify_ignoring_excl_unlink(InotifyMask::MODIFY);
        Ok(())
    }

    pub fn fallocate(
        &self,
        current_task: &CurrentTask,
        mode: FallocMode,
        offset: u64,
        length: u64,
    ) -> Result<(), Errno> {
        // If the file is a pipe or FIFO, ESPIPE is returned.
        // See https://man7.org/linux/man-pages/man2/fallocate.2.html#ERRORS
        if self.node().is_fifo() {
            return error!(ESPIPE);
        }

        // Must be a regular file or directory.
        // See https://man7.org/linux/man-pages/man2/fallocate.2.html#ERRORS
        if !self.node().is_dir() && !self.node().is_reg() {
            return error!(ENODEV);
        }

        // The file must be opened with write permissions. Otherwise operation is forbidden.
        // See https://man7.org/linux/man-pages/man2/fallocate.2.html#ERRORS
        if !self.can_write() {
            return error!(EBADF);
        }

        security::file_permission(current_task, self, security::PermissionFlags::WRITE)?;

        self.node().fallocate(current_task, mode, offset, length)?;
        self.notify(InotifyMask::MODIFY);
        Ok(())
    }

    pub fn to_handle(
        &self,
        current_task: &CurrentTask,
    ) -> Result<Option<zx::NullableHandle>, Errno> {
        self.ops().to_handle(self, current_task)
    }

    pub fn get_handles(
        &self,
        current_task: &CurrentTask,
    ) -> Result<Vec<zx::NullableHandle>, Errno> {
        self.ops().get_handles(self, current_task)
    }

    pub fn as_pid(&self) -> Result<Pid, Errno> {
        self.ops().as_pid(self)
    }

    /// Upgrades the weak reference to return the strong [`FileHandle`] referencing this
    /// [`FileObject`].
    pub fn to_file_handle(&self) -> FileHandle {
        self.weak_handle.upgrade().expect("FileObject is alive, so weak_handle must upgrade")
    }

    /// Creates a [`FileMapping`] for this file, acquiring the specified write guard mode on the
    /// node if any.
    pub fn to_mapping(&self, mode: Option<FileWriteGuardMode>) -> Result<Arc<FileMapping>, Errno> {
        FileMapping::new(self.to_file_handle(), mode)
    }

    /// Update the file flags.
    ///
    /// Writes the bits in `value` that are set in `mask` into the file flags.
    ///
    /// Does not provide any synchronization.
    pub fn update_file_flags(&self, value: OpenFlags, mask: OpenFlags) {
        self.flags.update(value, mask, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Get the async owner of this file.
    ///
    /// See fcntl(F_GETOWN)
    pub fn get_async_owner(&self) -> FileAsyncOwner {
        self.async_owner.lock().clone()
    }

    /// Set the async owner of this file.
    ///
    /// See fcntl(F_SETOWN)
    pub fn set_async_owner(&self, owner: FileAsyncOwner) {
        *self.async_owner.lock() = owner;
    }

    /// See fcntl(F_GETLEASE)
    pub fn get_lease(&self) -> FileLeaseType {
        *self.lease.lock()
    }

    /// See fcntl(F_SETLEASE)
    pub fn set_lease(&self, current_task: &CurrentTask, lease: FileLeaseType) -> Result<(), Errno> {
        if !self.node().is_reg() {
            return error!(EINVAL);
        }
        security::check_file_lock_access(current_task, self)?;
        if lease == FileLeaseType::Read && self.can_write() {
            return error!(EAGAIN);
        }
        *self.lease.lock() = lease;
        Ok(())
    }

    /// Wait on the specified events and call the EventHandler when ready
    pub fn wait_async(
        &self,
        current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        self.ops().wait_async(self, current_task, waiter, events, handler)
    }

    /// The events currently active on this file.
    pub fn query_events(&self, current_task: &CurrentTask) -> Result<FdEvents, Errno> {
        self.ops().query_events(self, current_task).map(FdEvents::add_equivalent_fd_events)
    }

    pub fn record_lock(
        &self,
        current_task: &CurrentTask,
        cmd: RecordLockCommand,
        flock: uapi::flock,
    ) -> Result<Option<uapi::flock>, Errno> {
        security::check_file_lock_access(current_task, self)?;
        self.node().record_lock(current_task, self, cmd, flock)
    }

    pub fn flush(&self, current_task: &CurrentTask, id: FdTableId) {
        self.name.entry.node.record_lock_release(RecordLockOwner::FdTable(id));
        self.ops().flush(self, current_task)
    }

    fn update_atime(&self) {
        if !self.flags().contains(OpenFlags::NOATIME) {
            self.name.update_atime();
        }
    }

    pub fn readahead(
        &self,
        current_task: &CurrentTask,
        offset: usize,
        length: usize,
    ) -> Result<(), Errno> {
        // readfile() fails with EBADF if the file was not open for read.
        if !self.can_read() {
            return error!(EBADF);
        }
        checked_add_offset_and_length(offset, length)?;
        self.ops().readahead(self, current_task, offset, length)
    }

    pub fn extra_fdinfo(&self, current_task: &CurrentTask) -> Option<FsString> {
        let file = self.weak_handle.upgrade()?;
        self.ops().extra_fdinfo(&file, current_task)
    }

    /// Register the fd number of an `EpollFileObject` that listens to events from this
    /// `FileObject`.
    pub fn register_epfd(&self, file: &FileHandle) {
        self.epoll_files.lock().insert(WeakKey::from(file), file.weak_handle.clone());
    }

    pub fn unregister_epfd(&self, file: &FileHandle) {
        self.epoll_files.lock().remove(&WeakKey::from(file));
    }
}

impl Releasable for FileObject {
    type Context<'a> = &'a CurrentTask;

    fn release<'a>(self, context: &'a CurrentTask) {
        let current_task = context;
        // Release all wake leases associated with this file in the corresponding `WaitObject`
        // of each registered epfd.
        for (_, file) in self.epoll_files.lock().drain() {
            if let Some(file) = file.upgrade() {
                if let Some(epoll_object) = file.downcast_file::<EpollFileObject>() {
                    let _ = epoll_object.delete(current_task, &self);
                }
            }
        }

        if self.can_write() {
            self.name.entry.node.write_guard_state.lock().release(FileWriteGuardMode::WriteFile);
        }

        let ops = self.ops;
        let state = self.state;
        ops.close(&state, current_task);
        state.name.entry.node.on_file_closed(&state);
        let event =
            if state.can_write() { InotifyMask::CLOSE_WRITE } else { InotifyMask::CLOSE_NOWRITE };
        state.notify(event);
    }
}

impl fmt::Debug for FileObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileObject")
            .field("name", &self.name)
            .field("fs", &self.fs.name())
            .field("offset", &self.offset)
            .field("flags", &self.flags)
            .field("ops_ty", &self.ops().type_name())
            .finish()
    }
}

impl OnWakeOps for FileReleaser {
    fn on_wake(&self, _current_task: &CurrentTask, _baton_lease: &zx::NullableHandle) {}
}

/// A FileObject with the type of its FileOps known. Dereferencing it returns the FileOps.
pub struct DowncastedFile<'a, Ops> {
    file: &'a FileObject,
    ops: &'a Ops,
}
impl<'a, Ops> Copy for DowncastedFile<'a, Ops> {}
impl<'a, Ops> Clone for DowncastedFile<'a, Ops> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, Ops> DowncastedFile<'a, Ops> {
    pub fn file(&self) -> &'a FileObject {
        self.file
    }
}

impl<'a, Ops> Deref for DowncastedFile<'a, Ops> {
    type Target = &'a Ops;
    fn deref(&self) -> &Self::Target {
        &self.ops
    }
}

impl FileObject {
    /// Returns the `FileObject`'s `FileOps` as a `DowncastedFile<T>`, or `None` if the downcast
    /// fails.
    ///
    /// This is useful for syscalls that only operate on a certain type of file.
    pub fn downcast_file<'a, T>(&'a self) -> Option<DowncastedFile<'a, T>>
    where
        T: 'static,
    {
        let ops = self.ops().as_any().downcast_ref::<T>()?;
        Some(DowncastedFile { file: self, ops })
    }
}

/// Invokes the specified one-way `method` on the `proxy` and waits until the `proxy`'s underlying
/// channel has been closed by the peer.
///
/// This is used in `close()` implementations when the `FileOps` wraps a FIDL resource that provides
/// a one-way API to request teardown, and acknowledges completion of teardown by closing the FIDL
/// channel, to ensure that the `close()` call does not return until the FIDL server has actually
/// processed the teardown request.
pub fn call_fidl_and_await_close<P, M>(method: M, proxy: &P)
where
    P: fidl::endpoints::SynchronousProxy,
    M: FnOnce(&P) -> Result<(), fidl::Error>,
{
    if let Err(e) = method(proxy) {
        log_error!("call_fidl_and_await_close: call {} failed: {e:?}", P::Protocol::DEBUG_NAME);
        return;
    }
    let channel = proxy.as_channel();
    let result = channel.wait_one(zx::Signals::CHANNEL_PEER_CLOSED, zx::MonotonicInstant::INFINITE);
    if let Err(status) = result.to_result() {
        log_error!(
            "call_fidl_and_await_close: wait_one {} failed: {status:?}",
            P::Protocol::DEBUG_NAME
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::fs::tmpfs::TmpFs;
    use crate::task::CurrentTask;
    use crate::task::dynamic_thread_spawner::SpawnRequestBuilder;
    use crate::testing::*;
    use crate::vfs::MountInfo;
    use crate::vfs::buffers::{VecInputBuffer, VecOutputBuffer};
    use starnix_uapi::auth::FsCred;
    use starnix_uapi::device_id::DeviceId;
    use starnix_uapi::file_mode::FileMode;
    use starnix_uapi::open_flags::OpenFlags;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use zerocopy::{FromBytes, IntoBytes, LE, U64};

    #[::fuchsia::test]
    async fn test_append_truncate_race() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let root_fs = TmpFs::new_fs(&kernel);
            let mount = MountInfo::detached();
            let root_node = Arc::clone(root_fs.root());
            let file = root_node
                .create_entry(&current_task, &mount, "test".into(), |dir, mount, name| {
                    dir.create_node(
                        &current_task,
                        mount,
                        name,
                        FileMode::IFREG | FileMode::ALLOW_ALL,
                        DeviceId::NONE,
                        FsCred::root(),
                    )
                })
                .expect("create_node failed");
            let file_handle = file
                .open_anonymous(&current_task, OpenFlags::APPEND | OpenFlags::RDWR)
                .expect("open failed");
            let done = Arc::new(AtomicBool::new(false));

            let fh = file_handle.clone();
            let done_clone = done.clone();
            let closure = move |current_task: &CurrentTask| {
                for i in 0..2000 {
                    fh.write(current_task, &mut VecInputBuffer::new(U64::<LE>::new(i).as_bytes()))
                        .expect("write failed");
                }
                done_clone.store(true, Ordering::SeqCst);
                let result: Result<(), starnix_uapi::errors::Errno> = Ok(());
                result
            };
            let (write_thread, req) =
                SpawnRequestBuilder::new().with_sync_closure(closure).build_with_sync_result();
            kernel.kthreads.spawner().spawn_from_request(req);

            let fh = file_handle.clone();
            let done_clone = done.clone();
            let closure = move |current_task: &CurrentTask| {
                while !done_clone.load(Ordering::SeqCst) {
                    fh.ftruncate(current_task, 0).expect("truncate failed");
                }
                let result: Result<(), starnix_uapi::errors::Errno> = Ok(());
                result
            };
            let (truncate_thread, req) =
                SpawnRequestBuilder::new().with_sync_closure(closure).build_with_sync_result();
            kernel.kthreads.spawner().spawn_from_request(req);

            // If we read from the file, we should always find an increasing sequence. If there are
            // races, then we might unexpectedly see zeroes.
            while !done.load(Ordering::SeqCst) {
                let mut buffer = VecOutputBuffer::new(4096);
                let amount =
                    file_handle.read_at(&current_task, 0, &mut buffer).expect("read failed");
                let mut last = None;
                let buffer = &Vec::from(buffer)[..amount];
                for i in
                    buffer.chunks_exact(8).map(|chunk| U64::<LE>::read_from_bytes(chunk).unwrap())
                {
                    if let Some(last) = last {
                        assert!(i.get() > last, "buffer: {:?}", buffer);
                    }
                    last = Some(i.get());
                }
            }

            let _ = write_thread().unwrap();
            let _ = truncate_thread().unwrap();
        })
        .await;
    }
}
