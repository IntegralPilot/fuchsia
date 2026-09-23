// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use linux_uapi::{
    UFFDIO_CONTINUE, UFFDIO_COPY, UFFDIO_WAKE, UFFDIO_WRITEPROTECT, UFFDIO_ZEROPAGE, uffdio_copy,
    uffdio_zeropage,
};
use starnix_core::mm::{
    FaultCopyMode, FaultRegisterMode, FaultZeroMode, MemoryAccessorExt, UserFault,
    UserFaultFeatures,
};
use starnix_core::task::{CurrentTask, EventHandler, WaitCanceler, Waiter};
use starnix_core::vfs::{
    Anon, FileHandle, FileObject, FileObjectState, FileOps, InputBuffer, OutputBuffer,
    fileops_impl_nonseekable, fileops_impl_noop_sync,
};
use starnix_logging::track_stub;

use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::UserRef;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{
    _UFFDIO_API, _UFFDIO_REGISTER, _UFFDIO_UNREGISTER, UFFDIO, UFFDIO_API, UFFDIO_MOVE,
    UFFDIO_POISON, UFFDIO_REGISTER, UFFDIO_UNREGISTER, errno, error, uapi, uffdio_api,
    uffdio_range, uffdio_register,
};
use static_assertions::const_assert_eq;
use std::sync::Arc;

uapi::check_arch_independent_layout! {
    uffdio_api {
        api,
        features,
        ioctls,
    }

    uffdio_range {
        start,
        len,
    }

    uffdio_register {
        range,
        mode,
        ioctls,
    }

    uffdio_copy {
        dst,
        src,
        len,
        mode,
        copy,
    }

    uffdio_zeropage {
        range,
        mode,
        zeropage,
    }

    uffdio_writeprotect {
        range,
        mode,
    }

    uffdio_continue {
        range,
        mode,
        mapped,
    }

    uffdio_poison {
        range,
        mode,
        updated,
    }

    uffdio_move {
        dst,
        src,
        len,
        mode,
        move_,
    }
}

pub struct UserFaultFile {
    inner: Arc<UserFault>,
}

// API version hasn't changed
const_assert_eq!(UFFDIO, 0xAA);

impl UserFaultFile {
    pub fn new(
        current_task: &CurrentTask,
        open_flags: OpenFlags,
        _user_mode_only: bool,
    ) -> Result<FileHandle, Errno> {
        let mm = current_task.mm()?;
        let inner = Arc::new(UserFault::new(Arc::downgrade(&mm)));
        mm.register_uffd(&inner);
        Anon::new_file(current_task, Box::new(Self { inner }), open_flags, "[userfaultfd]")
    }

    fn api_handshake(
        &self,
        _current_task: &CurrentTask,
        request: uffdio_api,
    ) -> Result<uffdio_api, Errno> {
        if self.inner.is_initialized() {
            return error!(EPERM, "userfault object already initialized");
        }

        if request.api != UFFDIO as u64 {
            return error!(EINVAL, format!("unsupported API version {}", request.api));
        }

        let requested_features =
            UserFaultFeatures::from_bits(request.features.try_into().map_err(|_| errno!(EINVAL))?)
                .ok_or_else(|| errno!(EINVAL))?;
        let requested_unsupported = requested_features.difference(UserFaultFeatures::ALL_SUPPORTED);
        if !requested_unsupported.is_empty() {
            return error!(EINVAL);
        }

        // We can support the client, initialize the object.
        self.inner.initialize(requested_features);

        Ok(uffdio_api {
            api: request.api,
            features: UserFaultFeatures::ALL_SUPPORTED.bits() as u64,
            ioctls: (1 << _UFFDIO_API) | (1 << _UFFDIO_REGISTER) | (1 << _UFFDIO_UNREGISTER),
        })
    }
}

impl FileOps for UserFaultFile {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();
    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        track_stub!(TODO("https://fxbug.dev/391599171"), "event-based uffd operations");
        error!(ENOTSUP)
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        error!(EINVAL)
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        track_stub!(TODO("https://fxbug.dev/391599171"), "event-based uffd operations");
        error!(ENOTSUP)
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _waiter: &Waiter,
        _events: FdEvents,
        _handler: EventHandler,
    ) -> Option<WaitCanceler> {
        track_stub!(TODO("https://fxbug.dev/391599171"), "event-based uffd operations");
        None
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: starnix_syscalls::SyscallArg,
    ) -> Result<starnix_syscalls::SyscallResult, Errno> {
        match request {
            UFFDIO_API => {
                let arg: UserRef<uffdio_api> = arg.into();
                let request = current_task.read_object(arg)?;
                match self.api_handshake(current_task, request) {
                    Ok(reply) => {
                        current_task.write_object(arg, &reply)?;
                        Ok(0.into())
                    }
                    Err(e) => {
                        current_task.write_object(arg, &uffdio_api::default())?;
                        Err(e)
                    }
                }
            }

            UFFDIO_REGISTER => {
                let arg: UserRef<uffdio_register> = arg.into();
                let mut request = current_task.read_object(arg)?;

                request.ioctls = self
                    .inner
                    .op_register(
                        request.range.start.into(),
                        request.range.len,
                        FaultRegisterMode::from_bits_truncate(
                            request.mode.try_into().map_err(|_| errno!(EINVAL))?,
                        ),
                    )?
                    .bits();
                current_task.write_object(arg, &request)?;
                Ok(0.into())
            }

            UFFDIO_UNREGISTER => {
                let arg: UserRef<uffdio_range> = arg.into();
                let request = current_task.read_object(arg)?;
                self.inner.op_unregister(request.start.into(), request.len)?;
                Ok(0.into())
            }

            UFFDIO_ZEROPAGE => {
                let arg: UserRef<uffdio_zeropage> = arg.into();
                let mut request = current_task.read_object(arg)?;
                let ioctl_res = self.inner.op_zero(
                    request.range.start.into(),
                    request.range.len,
                    FaultZeroMode::from_bits_truncate(
                        request.mode.try_into().map_err(|_| errno!(EINVAL))?,
                    ),
                );
                request.zeropage = match ioctl_res {
                    Ok(bytes) => bytes as i64,
                    Err(ref e) => -1 * (e.code.error_code() as i64),
                };
                current_task.write_object(arg, &request)?;
                // EAGAIN is returned if the number of bytes zeroed is not equal to the requested
                // length
                match ioctl_res {
                    Ok(bytes) if bytes == request.range.len as usize => Ok(0.into()),
                    Err(e) => Err(e),
                    _ => error!(EAGAIN),
                }
            }

            UFFDIO_COPY => {
                let arg: UserRef<uffdio_copy> = arg.into();
                let mut request = current_task.read_object(arg)?;
                let mm = current_task.mm()?;
                let ioctl_res = self.inner.op_copy(
                    &mm,
                    request.src.into(),
                    request.dst.into(),
                    request.len,
                    FaultCopyMode::from_bits_truncate(
                        request.mode.try_into().map_err(|_| errno!(EINVAL))?,
                    ),
                );
                request.copy = match ioctl_res {
                    Ok(bytes) => bytes as i64,
                    Err(ref e) => -1 * (e.code.error_code() as i64),
                };
                current_task.write_object(arg, &request)?;
                // EAGAIN is returned if the number of bytes copied is not equal to the requested
                // length
                match ioctl_res {
                    Ok(bytes) if bytes == request.len as usize => Ok(0.into()),
                    Err(e) => Err(e),
                    _ => error!(EAGAIN),
                }
            }

            UFFDIO_MOVE => {
                track_stub!(TODO("https://fxbug.dev/297375964"), "basic uffd ioctls", request);
                error!(ENOSYS)
            }

            UFFDIO_WAKE | UFFDIO_WRITEPROTECT | UFFDIO_CONTINUE | UFFDIO_POISON => {
                track_stub!(
                    TODO("https://fxbug.dev/322893681"),
                    "full set of uffd ioctls",
                    request
                );
                error!(ENOSYS)
            }

            unknown => error!(EINVAL, format!("unknown ioctl request {unknown}")),
        }
    }

    // On closing, clear all the registrations pointing to this userfault object.
    fn close(self: Box<Self>, _file: &FileObjectState, _current_task: &CurrentTask) {
        self.inner.cleanup();
    }
}
