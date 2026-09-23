// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_buildinfo as buildinfo;
use fuchsia_component::client::connect_to_protocol_sync;
use fuchsia_rcu::RcuReadScope;
use starnix_core::arch::{ARCH_NAME, ARCH_NAME32};
use starnix_core::mm::{MemoryAccessor, MemoryAccessorExt, PAGE_SIZE};
use starnix_core::security;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::FsString;
use starnix_logging::{log_error, track_stub};
use starnix_syscalls::decls::SyscallDecl;
use starnix_syscalls::{SUCCESS, SyscallResult};
use starnix_types::user_buffer::MAX_RW_COUNT;
use starnix_uapi::auth::{CAP_SYS_ADMIN, CAP_SYS_MODULE};
use starnix_uapi::errors::Errno;
use starnix_uapi::personality::PersonalityFlags;
use starnix_uapi::user_address::{MultiArchUserRef, UserAddress, UserCString, UserRef};
use starnix_uapi::version::KERNEL_RELEASE;
use starnix_uapi::{
    EFAULT, GRND_NONBLOCK, GRND_RANDOM, c_char, errno, error, from_status_like_fdio, uapi, utsname,
};

uapi::check_arch_independent_layout! {
    utsname {
        sysname,
        nodename,
        release,
        version,
        machine,
        domainname,
    }
}

pub fn do_uname(current_task: &CurrentTask, result: &mut utsname) -> Result<(), Errno> {
    fn init_array(fixed: &mut [c_char; 65], init: &[u8]) {
        let len = init.len();
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        fixed[..len].copy_from_slice(zerocopy::transmute_ref!(init))
    }

    init_array(&mut result.sysname, b"Linux");
    if current_task.thread_group().read().personality.contains(PersonalityFlags::UNAME26) {
        init_array(&mut result.release, b"2.6.40-starnix");
    } else {
        init_array(&mut result.release, KERNEL_RELEASE.as_bytes());
    }

    let version = current_task.kernel().build_version.get_or_try_init(|| {
        let proxy =
            connect_to_protocol_sync::<buildinfo::ProviderMarker>().map_err(|_| errno!(ENOENT))?;
        let buildinfo = proxy.get_build_info(zx::MonotonicInstant::INFINITE).map_err(|e| {
            log_error!("FIDL error getting build info: {e}");
            errno!(EIO)
        })?;
        Ok(buildinfo.version.unwrap_or_else(|| "starnix".to_string()))
    })?;

    init_array(&mut result.version, version.as_bytes());

    let personality = current_task.thread_group().read().personality;
    let machine = if personality.execution_domain() == (uapi::PER_LINUX32 as u32) {
        ARCH_NAME32
    } else {
        ARCH_NAME
    };
    init_array(&mut result.machine, machine);

    {
        // Get the UTS namespace from the perspective of this task.
        let task_state = current_task.read();
        let uts_ns = task_state.uts_ns.read();
        init_array(&mut result.nodename, uts_ns.hostname.as_slice());
        init_array(&mut result.domainname, uts_ns.domainname.as_slice());
    }
    Ok(())
}

pub fn sys_uname(current_task: &CurrentTask, name: UserRef<utsname>) -> Result<(), Errno> {
    let mut result = utsname {
        sysname: [0; 65],
        nodename: [0; 65],
        release: [0; 65],
        version: [0; 65],
        machine: [0; 65],
        domainname: [0; 65],
    };
    do_uname(current_task, &mut result)?;
    current_task.write_object(name, &result)?;
    Ok(())
}

pub fn sys_sysinfo(
    current_task: &CurrentTask,
    info: MultiArchUserRef<uapi::sysinfo, uapi::arch32::sysinfo>,
) -> Result<(), Errno> {
    let total_ram = zx::system_get_physmem();
    let num_procs = current_task.kernel().pids.get_thread_groups(&RcuReadScope::new()).count();

    track_stub!(TODO("https://fxbug.dev/297374270"), "compute system load");
    let loads = [0; 3];

    track_stub!(TODO("https://fxbug.dev/322874530"), "compute actual free ram usage");
    let freeram = total_ram / 8;

    let result = uapi::sysinfo {
        uptime: (zx::MonotonicInstant::get() - zx::MonotonicInstant::ZERO).into_seconds(),
        loads,
        totalram: total_ram,
        freeram,
        procs: num_procs.try_into().map_err(|_| errno!(EINVAL))?,
        mem_unit: 1,
        ..Default::default()
    };

    current_task.write_multi_arch_object(info, result)?;
    Ok(())
}

// Used to read a hostname or domainname from task memory
fn read_name(current_task: &CurrentTask, name: UserCString, len: u64) -> Result<FsString, Errno> {
    const MAX_LEN: usize = 64;
    let len = len as usize;

    if len > MAX_LEN {
        return error!(EINVAL);
    }

    // Read maximum characters and mark the null terminator.
    let mut name = current_task.read_c_string_to_vec(name, MAX_LEN)?;

    // Syscall may have specified an even smaller length, so trim to the requested length.
    if len < name.len() {
        name.truncate(len);
    }
    Ok(name)
}

pub fn sys_sethostname(
    current_task: &CurrentTask,
    hostname: UserCString,
    len: u64,
) -> Result<SyscallResult, Errno> {
    security::check_task_capable(current_task, CAP_SYS_ADMIN)?;

    let hostname = read_name(current_task, hostname, len)?;

    let task_state = current_task.read();
    let mut uts_ns = task_state.uts_ns.write();
    uts_ns.hostname = hostname;

    Ok(SUCCESS)
}

pub fn sys_setdomainname(
    current_task: &CurrentTask,
    domainname: UserCString,
    len: u64,
) -> Result<SyscallResult, Errno> {
    security::check_task_capable(current_task, CAP_SYS_ADMIN)?;

    let domainname = read_name(current_task, domainname, len)?;

    let task_state = current_task.read();
    let mut uts_ns = task_state.uts_ns.write();
    uts_ns.domainname = domainname;

    Ok(SUCCESS)
}

pub fn sys_getrandom(
    current_task: &CurrentTask,
    start_addr: UserAddress,
    size: usize,
    flags: u32,
) -> Result<usize, Errno> {
    if flags & !(GRND_RANDOM | GRND_NONBLOCK) != 0 {
        return error!(EINVAL);
    }

    // Copy random bytes in up-to-page-size chunks, stopping either when all the user-requested
    // space has been written to or when we fault.
    let mut bytes_written = 0;
    let mut bounce_buffer = vec![0u8; std::cmp::min(*PAGE_SIZE as usize, size)];

    let bytes_to_write = std::cmp::min(size, *MAX_RW_COUNT);

    while bytes_written < bytes_to_write {
        let chunk_start = start_addr.saturating_add(bytes_written);
        let chunk_len = std::cmp::min(*PAGE_SIZE as usize, size - bytes_written);

        let chunk = &mut bounce_buffer[..chunk_len];
        starnix_crypto::cprng_draw(chunk);
        match current_task.write_memory_partial(chunk_start, chunk) {
            Ok(n) => {
                bytes_written += n;

                // If we didn't write the whole chunk then we faulted. Don't try to write any more.
                if n < chunk_len {
                    break;
                }
            }

            // write_memory_partial fails if no bytes were written, but we might have
            // written bytes already.
            Err(e) if e.code.error_code() == EFAULT && bytes_written > 0 => break,
            Err(e) => return Err(e),
        }
    }

    Ok(bytes_written)
}

pub fn sys_sched_yield(_current_task: &CurrentTask) -> Result<(), Errno> {
    // SAFETY: This is unsafe because it is a syscall. zx_thread_legacy_yield is always safe.
    let status = unsafe { zx::sys::zx_thread_legacy_yield(0) };
    zx::Status::ok(status).map_err(|status| from_status_like_fdio!(status))
}

pub fn sys_unknown(
    current_task: &CurrentTask,
    syscall_number: u64,
) -> Result<SyscallResult, Errno> {
    let decl = SyscallDecl::from_number(syscall_number, current_task.thread_state.arch_width());
    track_stub!(TODO("https://fxbug.dev/322874143"), decl.name(), syscall_number);

    // TODO(https://fxbug.dev/454657040) We should send SIGSYS once we have signals.
    error!(ENOSYS)
}

pub fn sys_personality(current_task: &CurrentTask, persona: u32) -> Result<SyscallResult, Errno> {
    let mut state = current_task.task.thread_group().write();
    let previous_value = state.personality.update_from_syscall(persona);
    Ok(previous_value.into())
}

pub fn sys_delete_module(
    current_task: &CurrentTask,
    user_name: UserCString,
    _flags: u32,
) -> Result<SyscallResult, Errno> {
    security::check_task_capable(current_task, CAP_SYS_MODULE)?;
    // According to LTP test delete_module02.c
    const MODULE_NAME_LEN: usize = 64 - std::mem::size_of::<u64>();
    let _name = current_task.read_c_string_to_vec(user_name, MODULE_NAME_LEN)?;
    // We don't ever have any modules loaded.
    error!(ENOENT)
}

// Syscalls for arch32 usage
#[cfg(target_arch = "aarch64")]
mod arch32 {
    pub use super::{sys_sysinfo as sys_arch32_sysinfo, sys_uname as sys_arch32_uname};
}

#[cfg(target_arch = "aarch64")]
pub use arch32::*;
