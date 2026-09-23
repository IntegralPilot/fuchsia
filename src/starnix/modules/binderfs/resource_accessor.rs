// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_posix as fposix;
use fidl_fuchsia_starnix_binder as fbinder;
use zx;

use starnix_core::device::mem::new_null_file;
use starnix_core::fs::fuchsia::new_remote_file;
use starnix_core::mm::MemoryAccessor;
use starnix_core::task::{CurrentTask, Task};
use starnix_core::vfs::{FdFlags, FdNumber, FileHandle};
use starnix_logging::{log_trace, log_warn, with_zx_name};
use starnix_types::convert::IntoFidl;
use starnix_uapi::auth::Credentials;
use starnix_uapi::errors::{Errno, errno, errno_from_code, error};

use starnix_uapi::user_address::UserAddress;
use std::cell::Cell;
use std::mem::MaybeUninit;
use std::sync::Arc;

pub struct RemoteIoctl {
    pub ioctl_reads: Vec<fbinder::IoctlReadWrite>,
    pub ioctl_writes: Cell<Vec<fbinder::IoctlReadWrite>>,
    pub vmo: zx::Vmo,
}

/// Abstraction for accessing resources of a given process, whether it is a current process or a
/// remote one.
pub trait ResourceAccessor: std::fmt::Debug {
    // File related methods.
    fn close_files(&self, fds: Vec<FdNumber>) -> Result<(), Errno>;
    fn get_files_with_flags(
        &self,
        current_task: &CurrentTask,
        fds: Vec<FdNumber>,
    ) -> Result<Vec<(FileHandle, FdFlags)>, Errno>;
    fn add_files_with_flags(
        &self,
        current_task: &CurrentTask,
        files: Vec<(FileHandle, FdFlags)>,
        add_action: &mut dyn FnMut(FdNumber),
    ) -> Result<Vec<FdNumber>, Errno>;

    // Convenience method to allow passing a MemoryAccessor as a parameter.
    fn as_memory_accessor(&self) -> Option<&dyn MemoryAccessor>;
}

/// Return the `ResourceAccessor` to use to access the resources of `task`. If
/// `remote_resource_accessor` is not empty, the task is remote, and it should be used instead.
pub fn get_resource_accessor<'a>(
    task: &'a dyn ResourceAccessor,
    remote_resource_accessor: &'a Option<Arc<RemoteResourceAccessor>>,
) -> &'a dyn ResourceAccessor {
    if let Some(resource_accessor) = remote_resource_accessor {
        resource_accessor.as_ref()
    } else {
        task
    }
}

pub struct RemoteResourceAccessor {
    pub process: zx::Process,
    pub process_accessor: fbinder::ProcessAccessorSynchronousProxy,
    pub remote_creds: Arc<Credentials>,
}

impl RemoteResourceAccessor {
    fn run_file_request(
        &self,
        request: fbinder::FileRequest,
    ) -> Result<fbinder::FileResponse, Errno> {
        let result = self
            .process_accessor
            .file_request(request, zx::MonotonicInstant::INFINITE)
            .map_err(|_| errno!(ENOENT))?;
        result.map_err(|e| errno_from_code!(e.into_primitive() as i16))
    }

    fn with_remote_creds<F, T>(&self, current_task: &CurrentTask, f: F) -> Result<T, Errno>
    where
        F: FnOnce() -> Result<T, Errno>,
    {
        current_task.override_creds(self.remote_creds.clone(), f)
    }
}

impl std::fmt::Debug for RemoteResourceAccessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteResourceAccessor").finish()
    }
}

pub struct RemoteMemoryAccessor<'b> {
    pub remote_resource_accessor: Arc<RemoteResourceAccessor>,
    pub remote_ioctl: &'b RemoteIoctl,
}

impl<'b> RemoteMemoryAccessor<'b> {
    fn map_fidl_posix_errno(e: fposix::Errno) -> Errno {
        errno_from_code!(e.into_primitive() as i16)
    }

    // Remove top bits to safely handle HWASAN tagged addresses (top byte ignore on Aarch64).
    fn untag_address(&self, addr: UserAddress) -> zx::sys::zx_vaddr_t {
        #[cfg(target_arch = "aarch64")]
        {
            (addr.ptr() as u64 & 0x00FF_FFFF_FFFF_FFFF) as zx::sys::zx_vaddr_t
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            addr.ptr()
        }
    }
}

// The maximal size of buffers that zircon supports for process_{read|write}_memory.
const MAX_PROCESS_READ_WRITE_MEMORY_BUFFER_SIZE: usize = 64 * 1024 * 1024;

impl<'b> MemoryAccessor for RemoteMemoryAccessor<'b> {
    fn read_memory<'a>(
        &self,
        addr: UserAddress,
        unread_bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        let addr = self.untag_address(addr);
        let unread_bytes_ptr = unread_bytes.as_mut_ptr();
        let unread_bytes_len = unread_bytes.len();

        let mut current_offset = 0;
        while current_offset < unread_bytes_len {
            let current_addr = (addr + current_offset) as u64;
            let remaining_len = unread_bytes_len - current_offset;

            // Find an IoctlRead that covers current_addr
            if let Some(ioctl_read) = self
                .remote_ioctl
                .ioctl_reads
                .iter()
                .find(|r| current_addr >= r.address && (current_addr - r.address) < r.length)
            {
                let read_len = std::cmp::min(
                    remaining_len,
                    (ioctl_read.address + ioctl_read.length - current_addr) as usize,
                );
                let offset_in_vmo = ioctl_read.offset + (current_addr - ioctl_read.address);
                // SAFETY: `MaybeUninit<u8>` has the same layout as `u8`. Creating a mutable slice
                // of `u8` from a mutable slice of `MaybeUninit<u8>` is safe once all bytes are
                // initialized, which `vmo.read` will guarantee.
                let dest_slice = unsafe {
                    std::slice::from_raw_parts_mut(
                        unread_bytes_ptr.add(current_offset) as *mut u8,
                        read_len,
                    )
                };
                self.remote_ioctl
                    .vmo
                    .read(dest_slice, offset_in_vmo)
                    .map_err(|_| errno!(EFAULT))?;
                current_offset += read_len;
            } else {
                let read_len =
                    std::cmp::min(remaining_len, MAX_PROCESS_READ_WRITE_MEMORY_BUFFER_SIZE);
                let dest_slice = &mut unread_bytes[current_offset..current_offset + read_len];
                let (read_bytes, _) = self
                    .remote_resource_accessor
                    .process
                    .read_memory_uninit(current_addr as usize, dest_slice)
                    .map_err(|_| errno!(EFAULT))?;
                if read_bytes.is_empty() {
                    return error!(EFAULT);
                }
                current_offset += read_bytes.len();
            }
        }

        debug_assert_eq!(current_offset, unread_bytes_len);
        // SAFETY: [MaybeUninit<T>] and [T] have the same layout. All bytes have been
        // initialized.
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(unread_bytes_ptr as *mut u8, unread_bytes_len)
        };
        Ok(bytes)
    }

    fn read_memory_partial_until_null_byte<'a>(
        &self,
        _addr: UserAddress,
        _bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        error!(ENOTSUP)
    }

    fn read_memory_partial<'a>(
        &self,
        _addr: UserAddress,
        _bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        error!(ENOTSUP)
    }

    fn write_memory(&self, addr: UserAddress, bytes: &[u8]) -> Result<usize, Errno> {
        // No bytes to write.
        if bytes.is_empty() {
            return Ok(0);
        }

        let untagged_addr = self.untag_address(addr) as u64;

        // Writes are returned through ioctl, if there is space.
        let mut ioctl_writes = self.remote_ioctl.ioctl_writes.take();
        if ioctl_writes.len() < fbinder::MAX_IOCTL_WRITE_COUNT as usize {
            let last = ioctl_writes.last().unwrap_or(&fbinder::IoctlReadWrite {
                address: 0,
                offset: 0,
                length: 0,
            });
            let offset = last.offset + last.length;
            ioctl_writes.push(fbinder::IoctlReadWrite {
                address: untagged_addr,
                offset,
                length: bytes.len() as u64,
            });
            self.remote_ioctl.ioctl_writes.set(ioctl_writes);
            self.remote_ioctl.vmo.write(bytes, offset).map_err(|_| errno!(ENOENT))?;
            return Ok(bytes.len());
        }
        self.remote_ioctl.ioctl_writes.set(ioctl_writes);
        // Otherwise use ProcessAccessor to write to the process.
        if bytes.len() <= fbinder::MAX_WRITE_BYTES as usize {
            self.remote_resource_accessor.process_accessor.write_bytes(
                untagged_addr,
                bytes,
                zx::MonotonicInstant::INFINITE,
            )
        } else {
            let vmo = with_zx_name(
                zx::Vmo::create(bytes.len() as u64).map_err(|_| errno!(EINVAL))?,
                b"starnix:device_binder",
            );
            vmo.write(bytes, 0).map_err(|_| errno!(EFAULT))?;
            vmo.set_content_size(&(bytes.len() as u64)).map_err(|_| errno!(EINVAL))?;
            self.remote_resource_accessor.process_accessor.write_memory(
                untagged_addr,
                vmo,
                zx::MonotonicInstant::INFINITE,
            )
        }
        .map_err(|_| errno!(ENOENT))?
        .map_err(Self::map_fidl_posix_errno)?;
        Ok(bytes.len())
    }

    fn write_memory_partial(&self, _addr: UserAddress, _bytes: &[u8]) -> Result<usize, Errno> {
        error!(ENOTSUP)
    }

    fn zero(&self, _addr: UserAddress, _length: usize) -> Result<usize, Errno> {
        error!(ENOTSUP)
    }
}

impl ResourceAccessor for RemoteResourceAccessor {
    fn close_files(&self, fds: Vec<FdNumber>) -> Result<(), Errno> {
        for chunk in fds.chunks(fbinder::MAX_REQUEST_COUNT as usize) {
            self.run_file_request(fbinder::FileRequest {
                close_requests: Some(chunk.iter().map(|fd| fd.raw()).collect()),
                ..Default::default()
            })?;
        }
        Ok(())
    }

    fn get_files_with_flags(
        &self,
        current_task: &CurrentTask,
        fds: Vec<FdNumber>,
    ) -> Result<Vec<(FileHandle, FdFlags)>, Errno> {
        let num_fds = fds.len();
        let mut files = Vec::with_capacity(num_fds);

        self.with_remote_creds(current_task, || {
            for chunk in fds.chunks(fbinder::MAX_REQUEST_COUNT as usize) {
                let response = self.run_file_request(fbinder::FileRequest {
                    get_requests: Some(chunk.iter().map(|fd| fd.raw()).collect()),
                    ..Default::default()
                })?;
                for fbinder::FileHandle { handle, flags, bag, .. } in
                    response.get_responses.into_iter().flatten()
                {
                    let Some(flags) = flags else {
                        log_warn!("Incorrect response to file request. Missing flags.");
                        return error!(ENOENT);
                    };
                    let file = if let Some(handle) = handle {
                        new_remote_file(current_task, handle, flags.into_fidl())?
                    } else if let Some(_bag) = bag {
                        // TODO(https://fxbug.dev/481167098): Support composite file descriptors.
                        log_warn!("FileHandle::bag is not supported");
                        return error!(EBADFD);
                    } else {
                        new_null_file(current_task, flags.into_fidl())
                    };
                    files.push((file, FdFlags::empty()));
                }
            }

            if files.len() != num_fds { error!(ENOENT) } else { Ok(files) }
        })
    }

    fn add_files_with_flags(
        &self,
        current_task: &CurrentTask,
        files: Vec<(FileHandle, FdFlags)>,
        add_action: &mut dyn FnMut(FdNumber),
    ) -> Result<Vec<FdNumber>, Errno> {
        let num_files = files.len();
        let mut fds = Vec::with_capacity(num_files);

        self.with_remote_creds(current_task, || {
            for chunk in files.chunks(fbinder::MAX_REQUEST_COUNT as usize) {
                let mut handles = Vec::with_capacity(chunk.len());
                for (file, _) in chunk.iter() {
                    let handle = file.to_handle(current_task);
                    let bag =
                        if handle.is_err() { Some(file.get_handles(current_task)?) } else { None };
                    handles.push(fbinder::FileHandle {
                        handle: handle.ok().flatten(),
                        bag,
                        // NOTE: We do not pass flags when adding files to a Fuchsia process as:
                        //   1. The flags are already set on the underlying fuchsia.io file.
                        //   2. There is only one flag (append) that you can set after the fact.
                        ..fbinder::FileHandle::default()
                    });
                }
                let response = self.run_file_request(fbinder::FileRequest {
                    add_requests: Some(handles),
                    ..Default::default()
                })?;
                for fd in
                    response.add_responses.into_iter().flatten().map(|fd| FdNumber::from_raw(fd))
                {
                    add_action(fd);
                    fds.push(fd);
                }
            }

            if fds.len() != num_files { error!(ENOENT) } else { Ok(fds) }
        })
    }

    fn as_memory_accessor(&self) -> Option<&dyn MemoryAccessor> {
        None
    }
}

/// Implementation of `ResourceAccessor` for a local client represented as a `CurrentTask`.
impl ResourceAccessor for CurrentTask {
    fn close_files(&self, fds: Vec<FdNumber>) -> Result<(), Errno> {
        for fd in fds {
            log_trace!("Closing fd {:?}", fd);
            self.files().close(fd)?;
        }
        Ok(())
    }

    fn get_files_with_flags(
        &self,
        _current_task: &CurrentTask,
        fds: Vec<FdNumber>,
    ) -> Result<Vec<(FileHandle, FdFlags)>, Errno> {
        let mut files = Vec::with_capacity(fds.len());
        for fd in fds {
            log_trace!("Getting file {:?} with flags", fd);
            // TODO: Should we allow O_PATH here?
            files.push(self.files().get_allowing_opath_with_flags(fd)?);
        }
        Ok(files)
    }

    fn add_files_with_flags(
        &self,
        current_task: &CurrentTask,
        files: Vec<(FileHandle, FdFlags)>,
        add_action: &mut dyn FnMut(FdNumber),
    ) -> Result<Vec<FdNumber>, Errno> {
        let mut fds = Vec::with_capacity(files.len());
        for (file, flags) in files {
            log_trace!("Adding file {:?} with flags {:?}", file, flags);
            let fd = self.files().add(current_task, file, flags)?;
            add_action(fd);
            fds.push(fd);
        }
        Ok(fds)
    }

    fn as_memory_accessor(&self) -> Option<&dyn MemoryAccessor> {
        Some(self)
    }
}

/// Implementation of `ResourceAccessor` for a local client represented as a `Task`.
impl ResourceAccessor for Task {
    fn close_files(&self, fds: Vec<FdNumber>) -> Result<(), Errno> {
        for fd in fds {
            log_trace!("Closing fd {:?}", fd);
            self.files()?.close(fd)?;
        }
        Ok(())
    }

    fn get_files_with_flags(
        &self,
        _current_task: &CurrentTask,
        fds: Vec<FdNumber>,
    ) -> Result<Vec<(FileHandle, FdFlags)>, Errno> {
        let mut files = Vec::with_capacity(fds.len());
        for fd in fds {
            log_trace!("Getting file {:?} with flags", fd);
            // TODO: Should we allow O_PATH here?
            files.push(self.files()?.get_allowing_opath_with_flags(fd)?);
        }
        Ok(files)
    }

    fn add_files_with_flags(
        &self,
        current_task: &CurrentTask,
        files: Vec<(FileHandle, FdFlags)>,
        add_action: &mut dyn FnMut(FdNumber),
    ) -> Result<Vec<FdNumber>, Errno> {
        let mut fds = Vec::with_capacity(files.len());
        for (file, flags) in files {
            log_trace!("Adding file {:?} with flags {:?}", file, flags);
            let fd = self.files()?.add(current_task, file, flags)?;
            add_action(fd);
            fds.push(fd);
        }
        Ok(fds)
    }

    fn as_memory_accessor(&self) -> Option<&dyn MemoryAccessor> {
        Some(self)
    }
}
