// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::io_uring::{IORING_MAX_ENTRIES, IoUringFileObject};
use starnix_core::mm::{IOVecPtr, MemoryAccessorExt};
use starnix_core::security;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::{FdFlags, FdNumber};
use starnix_logging::track_stub;

use starnix_syscalls::{SUCCESS, SyscallResult};
use starnix_uapi::auth::CAP_SYS_ADMIN;
use starnix_uapi::errors::Errno;
use starnix_uapi::signals::SigSet;
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::user_value::UserValue;
use starnix_uapi::{
    errno, error, io_uring_params,
    io_uring_register_op_IORING_REGISTER_BUFFERS as IORING_REGISTER_BUFFERS,
    io_uring_register_op_IORING_REGISTER_IOWQ_MAX_WORKERS as IORING_REGISTER_IOWQ_MAX_WORKERS,
    io_uring_register_op_IORING_REGISTER_PBUF_RING as IORING_REGISTER_PBUF_RING,
    io_uring_register_op_IORING_REGISTER_PBUF_STATUS as IORING_REGISTER_PBUF_STATUS,
    io_uring_register_op_IORING_REGISTER_PERSONALITY as IORING_REGISTER_PERSONALITY,
    io_uring_register_op_IORING_REGISTER_RING_FDS as IORING_REGISTER_RING_FDS,
    io_uring_register_op_IORING_UNREGISTER_BUFFERS as IORING_UNREGISTER_BUFFERS,
    io_uring_register_op_IORING_UNREGISTER_PBUF_RING as IORING_UNREGISTER_PBUF_RING,
    io_uring_register_op_IORING_UNREGISTER_RING_FDS as IORING_UNREGISTER_RING_FDS, uapi,
};
use std::sync::atomic;

pub fn sys_io_uring_setup(
    current_task: &CurrentTask,
    user_entries: UserValue<u32>,
    user_params: UserRef<io_uring_params>,
) -> Result<FdNumber, Errno> {
    // TODO: https://fxbug.dev/397186254 - we will want to do a no-audit CAP_IPC_LOCK capability
    // check; see "If not granted CAP_IPC_LOCK io_uring operations are accounted against the user's
    // RLIMIT_MEMLOCK limit" at
    // https://github.com/SELinuxProject/selinux-notebook/blob/main/src/auditing.md#capability-audit-exemptions

    if !current_task.kernel().features.io_uring {
        return error!(ENOSYS);
    }

    // Apply policy from /proc/sys/kernel/io_uring_disabled
    let limits = &current_task.kernel().system_limits;
    match limits.io_uring_disabled.load(atomic::Ordering::Relaxed) {
        0 => (),
        1 => {
            let io_uring_group = limits.io_uring_group.load(atomic::Ordering::Relaxed).try_into();
            if io_uring_group.is_err()
                || !current_task.current_creds().is_in_group(io_uring_group.unwrap())
            {
                security::check_task_capable(current_task, CAP_SYS_ADMIN)?;
            }
        }
        _ => {
            return error!(EPERM);
        }
    }

    let entries = user_entries.validate(1..IORING_MAX_ENTRIES).ok_or_else(|| errno!(EINVAL))?;

    let mut params = current_task.read_object(user_params)?;
    for byte in params.resv {
        if byte != 0 {
            return error!(EINVAL);
        }
    }

    let file = IoUringFileObject::new_file(current_task, entries, &mut params)?;

    // io_uring file descriptors are always created with CLOEXEC.
    let fd = current_task.add_file(file, FdFlags::CLOEXEC)?;
    current_task.write_object(user_params, &params)?;
    Ok(fd)
}

pub fn sys_io_uring_enter(
    current_task: &CurrentTask,
    fd: FdNumber,
    to_submit: u32,
    min_complete: u32,
    flags: u32,
    _sig: UserRef<SigSet>,
    sigset_size: usize,
) -> Result<u32, Errno> {
    if !current_task.kernel().features.io_uring {
        return error!(ENOSYS);
    }
    if !_sig.is_null() {
        if sigset_size != std::mem::size_of::<SigSet>() {
            return error!(EINVAL);
        }
    }
    let file = current_task.files().get(fd)?;
    let io_uring = file.downcast_file::<IoUringFileObject>().ok_or_else(|| errno!(EOPNOTSUPP))?;
    // TODO(https://fxbug.dev/297431387): Use `_sig` to change the signal mask for `current_task`.
    io_uring.enter(current_task, to_submit, min_complete, flags)
}

pub fn sys_io_uring_register(
    current_task: &CurrentTask,
    fd: FdNumber,
    opcode: u32,
    arg: UserAddress,
    nr_args: UserValue<u32>,
) -> Result<SyscallResult, Errno> {
    if !current_task.kernel().features.io_uring {
        return error!(ENOSYS);
    }
    let file = current_task.files().get(fd)?;
    let io_uring = file.downcast_file::<IoUringFileObject>().ok_or_else(|| errno!(EOPNOTSUPP))?;
    match opcode {
        IORING_REGISTER_BUFFERS => {
            // TODO(https://fxbug.dev/297431387): Check nr_args for zero and return EINVAL here.
            let iovec = IOVecPtr::new(current_task, arg);
            let buffers = current_task.read_iovec(iovec, nr_args)?;
            io_uring.register_buffers(buffers);
            return Ok(SUCCESS);
        }
        IORING_UNREGISTER_BUFFERS => {
            if !arg.is_null() {
                return error!(EINVAL);
            }
            io_uring.unregister_buffers();
            return Ok(SUCCESS);
        }
        IORING_REGISTER_IOWQ_MAX_WORKERS => {
            track_stub!(
                TODO("https://fxbug.dev/297431387"),
                "io_uring_register IORING_REGISTER_IOWQ_MAX_WORKERS",
                opcode
            );
            // The current implementation only ever use 1 worker for read and 1 for write.
            return Ok(SUCCESS);
        }
        IORING_REGISTER_RING_FDS => {
            track_stub!(
                TODO("https://fxbug.dev/297431387"),
                "io_uring_register IORING_REGISTER_RING_FDS",
                opcode
            );
            // The current implementation doesn't use any thread local specific identifier for
            // performance. Instead, when registering a fd, just return the passed fd as the value
            // to use.
            let nr_args: usize = nr_args.raw().try_into().map_err(|_| errno!(EINVAL))?;
            if nr_args > 16 {
                return error!(EINVAL);
            }
            let updates_addr = UserRef::<uapi::io_uring_rsrc_update>::from(arg);
            let mut updates = current_task
                .read_objects_to_smallvec::<uapi::io_uring_rsrc_update, 1>(updates_addr, nr_args)?;
            let mut result = 0;
            for update in updates.iter_mut() {
                if update.offset == u32::MAX {
                    update.offset = update.data.try_into().map_err(|_| errno!(EINVAL))?;
                    result += 1;
                }
            }
            current_task.write_objects(updates_addr, &updates)?;
            return Ok(result.into());
        }
        IORING_UNREGISTER_RING_FDS => {
            track_stub!(
                TODO("https://fxbug.dev/297431387"),
                "io_uring_register IORING_UNREGISTER_RING_FDS",
                opcode
            );
            // Because registering a fd doesn't use any resource currently, unregistering is free.
            return Ok(SUCCESS);
        }
        IORING_REGISTER_PBUF_RING => {
            let nr_args: usize = nr_args.raw().try_into().map_err(|_| errno!(EINVAL))?;
            if nr_args != 1 {
                return error!(EINVAL);
            }
            let buffer_definition: uapi::io_uring_buf_reg = current_task.read_object(arg.into())?;
            io_uring.register_ring_buffers(buffer_definition)?;
            return Ok(SUCCESS);
        }

        IORING_UNREGISTER_PBUF_RING => {
            let nr_args: usize = nr_args.raw().try_into().map_err(|_| errno!(EINVAL))?;
            if nr_args != 1 {
                return error!(EINVAL);
            }
            let buffer_definition: uapi::io_uring_buf_reg = current_task.read_object(arg.into())?;
            io_uring.unregister_ring_buffers(buffer_definition)?;
            return Ok(SUCCESS);
        }

        IORING_REGISTER_PBUF_STATUS => {
            let nr_args: usize = nr_args.raw().try_into().map_err(|_| errno!(EINVAL))?;
            if nr_args != 1 {
                return error!(EINVAL);
            }
            let buffer_status_addr = UserRef::<uapi::io_uring_buf_status>::from(arg);
            let mut buffer_status: uapi::io_uring_buf_status =
                current_task.read_object(buffer_status_addr)?;
            io_uring.ring_buffer_status(&mut buffer_status)?;
            current_task.write_object(buffer_status_addr, &buffer_status)?;
            return Ok(SUCCESS);
        }
        IORING_REGISTER_PERSONALITY => {
            // TODO(https://fxbug.dev/505326006) If registering personality is implemented,
            // then implement the uring_override_creds security hook.
            track_stub!(
                TODO("https://fxbug.dev/297431387"),
                "io_uring_register unknown op",
                opcode
            );
            return error!(EINVAL);
        }
        _ => {
            track_stub!(
                TODO("https://fxbug.dev/297431387"),
                "io_uring_register unknown op",
                opcode
            );
            return error!(EINVAL);
        }
    }
}

pub use sys_io_uring_enter as sys_arch32_io_uring_enter;
pub use sys_io_uring_register as sys_arch32_io_uring_register;
pub use sys_io_uring_setup as sys_arch32_io_uring_setup;
