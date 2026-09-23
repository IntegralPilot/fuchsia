// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::barrier::{BarrierType, system_barrier};
use crate::mm::debugger::notify_debugger_of_module_list;
use crate::mm::{
    DesiredAddress, FutexKey, IOVecPtr, MappingName, MappingOptions, MembarrierType,
    MemoryAccessorExt, MremapFlags, MsyncFlags, PAGE_SIZE, PrivateFutexKey, ProtectionFlags,
    SharedFutexKey,
};
use crate::security;
use crate::syscalls::time::TimeSpecPtr;
use crate::task::CurrentTask;
use crate::time::TargetTime;
use crate::time::utc::estimate_boot_deadline_from_utc;
use crate::vfs::FdNumber;
use crate::vfs::buffers::{OutputBuffer, UserBuffersInputBuffer, UserBuffersOutputBuffer};
use fuchsia_runtime::UtcTimeline;
use linux_uapi::MLOCK_ONFAULT;
use starnix_logging::{CATEGORY_STARNIX_MM, log_trace, track_stub};
use starnix_syscalls::SyscallArg;
use starnix_types::time::{duration_from_timespec, time_from_timespec, timespec_from_time};
use starnix_uapi::auth::{PTRACE_MODE_ATTACH_REALCREDS, PTRACE_MODE_READ_REALCREDS};
use starnix_uapi::errors::{EINTR, Errno};
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::user_value::UserValue;
use starnix_uapi::{
    FUTEX_BITSET_MATCH_ANY, FUTEX_CLOCK_REALTIME, FUTEX_CMD_MASK, FUTEX_CMP_REQUEUE,
    FUTEX_CMP_REQUEUE_PI, FUTEX_LOCK_PI, FUTEX_LOCK_PI2, FUTEX_PRIVATE_FLAG, FUTEX_REQUEUE,
    FUTEX_TRYLOCK_PI, FUTEX_UNLOCK_PI, FUTEX_WAIT, FUTEX_WAIT_BITSET, FUTEX_WAIT_REQUEUE_PI,
    FUTEX_WAKE, FUTEX_WAKE_BITSET, FUTEX_WAKE_OP, MAP_ANONYMOUS, MAP_DENYWRITE, MAP_FIXED,
    MAP_FIXED_NOREPLACE, MAP_GROWSDOWN, MAP_LOCKED, MAP_NORESERVE, MAP_POPULATE, MAP_PRIVATE,
    MAP_SHARED, MAP_SHARED_VALIDATE, MAP_STACK, PROT_EXEC, errno, error, robust_list_head, tid_t,
    uapi,
};
use std::ops::Deref as _;
use zx;

#[cfg(target_arch = "x86_64")]
use starnix_uapi::MAP_32BIT;

// Returns any platform-specific mmap flags. This is a separate function because as of this writing
// "attributes on expressions are experimental."
#[cfg(target_arch = "x86_64")]
fn get_valid_platform_mmap_flags() -> u32 {
    MAP_32BIT
}
#[cfg(not(target_arch = "x86_64"))]
fn get_valid_platform_mmap_flags() -> u32 {
    0
}

/// sys_mmap takes a mutable reference to current_task because it may modify the IP register.
pub fn sys_mmap(
    current_task: &mut CurrentTask,
    addr: UserAddress,
    length: usize,
    prot: u32,
    flags: u32,
    fd: FdNumber,
    offset: u64,
) -> Result<UserAddress, Errno> {
    let user_address = do_mmap(current_task, addr, length, prot, flags, fd, offset)?;
    if prot & PROT_EXEC != 0 {
        // Possibly loads a new module. Notify debugger for the change.
        // We only care about dynamic linker loading modules for now, which uses mmap. In the future
        // we might want to support unloading modules in munmap or JIT compilation in mprotect.
        notify_debugger_of_module_list(current_task)?;
    }
    Ok(user_address)
}

pub fn do_mmap(
    current_task: &CurrentTask,
    addr: UserAddress,
    length: usize,
    prot: u32,
    flags: u32,
    fd: FdNumber,
    offset: u64,
) -> Result<UserAddress, Errno> {
    let prot_flags = ProtectionFlags::from_access_bits(prot).ok_or_else(|| {
        track_stub!(TODO("https://fxbug.dev/322874211"), "mmap parse protection", prot);
        errno!(EINVAL)
    })?;

    let valid_flags: u32 = get_valid_platform_mmap_flags()
        | MAP_PRIVATE
        | MAP_SHARED
        | MAP_SHARED_VALIDATE
        | MAP_ANONYMOUS
        | MAP_FIXED
        | MAP_FIXED_NOREPLACE
        | MAP_POPULATE
        | MAP_NORESERVE
        | MAP_STACK
        | MAP_DENYWRITE
        | MAP_GROWSDOWN
        | MAP_LOCKED;
    if flags & !valid_flags != 0 {
        if flags & MAP_SHARED_VALIDATE != 0 {
            return error!(EOPNOTSUPP);
        }
        track_stub!(TODO("https://fxbug.dev/322873638"), "mmap check flags", flags);
        return error!(EINVAL);
    }

    let file = if flags & MAP_ANONYMOUS != 0 { None } else { Some(current_task.files().get(fd)?) };
    if flags & (MAP_PRIVATE | MAP_SHARED) == 0
        || flags & (MAP_PRIVATE | MAP_SHARED) == MAP_PRIVATE | MAP_SHARED
    {
        return error!(EINVAL);
    }
    if length == 0 {
        return error!(EINVAL);
    }
    if offset % *PAGE_SIZE != 0 {
        return error!(EINVAL);
    }

    let page_size = *PAGE_SIZE as usize;
    let length_aligned =
        length.checked_add(page_size - 1).ok_or_else(|| errno!(ENOMEM))? & !(page_size - 1);
    let rlimit_as = current_task
        .thread_group()
        .get_rlimit(starnix_uapi::resource_limits::Resource::AS) as usize;
    let current_usage: usize = current_task.mm()?.get_total_usage();

    if current_usage.saturating_add(length_aligned) > rlimit_as {
        return error!(ENOMEM);
    }

    // TODO(tbodt): should we consider MAP_NORESERVE?

    let addr = match (addr, flags & MAP_FIXED != 0, flags & MAP_FIXED_NOREPLACE != 0) {
        (UserAddress::NULL, false, false) => DesiredAddress::Any,
        (UserAddress::NULL, true, _) | (UserAddress::NULL, _, true) => return error!(EINVAL),
        (addr, false, false) => DesiredAddress::Hint(addr),
        (addr, _, true) => DesiredAddress::Fixed(addr),
        (addr, true, false) => DesiredAddress::FixedOverwrite(addr),
    };

    let memory_offset = if flags & MAP_ANONYMOUS != 0 { 0 } else { offset };

    let mut options = MappingOptions::empty();
    if flags & MAP_SHARED != 0 {
        options |= MappingOptions::SHARED;
    }
    if flags & MAP_ANONYMOUS != 0 {
        options |= MappingOptions::ANONYMOUS;
    }
    #[cfg(target_arch = "x86_64")]
    if flags & MAP_FIXED == 0 && flags & MAP_32BIT != 0 {
        options |= MappingOptions::LOWER_32BIT;
    }
    if flags & MAP_GROWSDOWN != 0 {
        options |= MappingOptions::GROWSDOWN;
    }
    if flags & MAP_POPULATE != 0 {
        options |= MappingOptions::POPULATE;
    }
    if flags & MAP_LOCKED != 0 {
        // The kernel isn't expected to return an error if locking fails with this flag, so for now
        // this implementation will always fail to lock memory even if mapping succeeds.
        track_stub!(TODO("https://fxbug.dev/406377606"), "MAP_LOCKED");
    }

    security::mmap_file(current_task, file.as_ref(), prot_flags, options)?;

    if flags & MAP_ANONYMOUS != 0 {
        fuchsia_trace::duration!(CATEGORY_STARNIX_MM, "AnonymousMmap");
        current_task.mm()?.map_anonymous(addr, length, prot_flags, options, MappingName::None)
    } else {
        fuchsia_trace::duration!(CATEGORY_STARNIX_MM, "FileBackedMmap");
        // TODO(tbodt): maximize protection flags so that mprotect works
        let file = file.expect("file retrieved above for file-backed mapping");
        file.mmap(current_task, addr, memory_offset, length, prot_flags, options)
    }
}

pub fn sys_mprotect(
    current_task: &CurrentTask,
    addr: UserAddress,
    length: usize,
    prot: u32,
) -> Result<(), Errno> {
    let prot_flags = ProtectionFlags::from_bits(prot).ok_or_else(|| {
        track_stub!(TODO("https://fxbug.dev/322874672"), "mprotect parse protection", prot);
        errno!(EINVAL)
    })?;
    current_task.mm()?.protect(current_task, addr, length, prot_flags)?;
    Ok(())
}

pub fn sys_mremap(
    current_task: &CurrentTask,
    addr: UserAddress,
    old_length: usize,
    new_length: usize,
    flags: u32,
    new_addr: UserAddress,
) -> Result<UserAddress, Errno> {
    let flags = MremapFlags::from_bits(flags).ok_or_else(|| errno!(EINVAL))?;
    let addr =
        current_task.mm()?.remap(current_task, addr, old_length, new_length, flags, new_addr)?;
    Ok(addr)
}

pub fn sys_munmap(
    current_task: &CurrentTask,
    addr: UserAddress,
    length: usize,
) -> Result<(), Errno> {
    current_task.mm()?.unmap(addr, length)?;
    Ok(())
}

pub fn sys_msync(
    current_task: &CurrentTask,
    addr: UserAddress,
    length: usize,
    flags: u32,
) -> Result<(), Errno> {
    let flags = MsyncFlags::from_bits_retain(flags);
    current_task.mm()?.msync(current_task, addr, length, flags)
}

pub fn sys_madvise(
    current_task: &CurrentTask,
    addr: UserAddress,
    length: usize,
    advice: u32,
) -> Result<(), Errno> {
    current_task.mm()?.madvise(addr, length, advice)?;
    Ok(())
}

pub fn sys_process_madvise(
    _current_task: &CurrentTask,
    _pidfd: FdNumber,
    _iovec_addr: IOVecPtr,
    _iovec_count: UserValue<i32>,
    _advice: UserValue<i32>,
    _flags: UserValue<u32>,
) -> Result<usize, Errno> {
    track_stub!(TODO("https://fxbug.dev/409060664"), "process_madvise");
    error!(ENOSYS)
}

pub fn sys_brk(current_task: &CurrentTask, addr: UserAddress) -> Result<UserAddress, Errno> {
    current_task.mm()?.set_brk(current_task, addr)
}

pub fn sys_process_vm_readv(
    current_task: &CurrentTask,
    tid: tid_t,
    local_iov_addr: IOVecPtr,
    local_iov_count: UserValue<i32>,
    remote_iov_addr: IOVecPtr,
    remote_iov_count: UserValue<i32>,
    flags: usize,
) -> Result<usize, Errno> {
    if flags != 0 {
        return error!(EINVAL);
    }

    // Source and destination are allowed to be of different length. It is valid to use a nullptr if
    // the associated length is 0. Thus, if either source or destination length is 0 and nullptr,
    // make sure to return Ok(0) before doing any other validation/operations.
    if (local_iov_count == 0 && local_iov_addr.is_null())
        || (remote_iov_count == 0 && remote_iov_addr.is_null())
    {
        return Ok(0);
    }

    let remote_task = current_task.get_task(tid)?;

    current_task.check_ptrace_access_mode(PTRACE_MODE_ATTACH_REALCREDS, &remote_task)?;

    let local_iov = current_task.read_iovec(local_iov_addr, local_iov_count)?;
    let remote_iov = current_task.read_iovec(remote_iov_addr, remote_iov_count)?;
    log_trace!(
        "process_vm_readv(tid={}, local_iov={:?}, remote_iov={:?})",
        tid,
        local_iov,
        remote_iov
    );

    track_stub!(TODO("https://fxbug.dev/322874765"), "process_vm_readv single-copy");
    // According to the man page, this syscall was added to Linux specifically to
    // avoid doing two copies like other IPC mechanisms require. We should avoid this too at some
    // point.
    let mut output = UserBuffersOutputBuffer::unified_new(current_task, local_iov)?;
    let remote_mm = remote_task.mm().ok();
    if current_task.has_same_address_space(remote_mm.as_ref()) {
        let mut input = UserBuffersInputBuffer::unified_new(current_task, remote_iov)?;
        output.write_buffer(&mut input)
    } else {
        let mut input = UserBuffersInputBuffer::syscall_new(remote_task.deref(), remote_iov)?;
        output.write_buffer(&mut input)
    }
}

pub fn sys_process_vm_writev(
    current_task: &CurrentTask,
    tid: tid_t,
    local_iov_addr: IOVecPtr,
    local_iov_count: UserValue<i32>,
    remote_iov_addr: IOVecPtr,
    remote_iov_count: UserValue<i32>,
    flags: usize,
) -> Result<usize, Errno> {
    if flags != 0 {
        return error!(EINVAL);
    }

    // Source and destination are allowed to be of different length. It is valid to use a nullptr if
    // the associated length is 0. Thus, if either source or destination length is 0 and nullptr,
    // make sure to return Ok(0) before doing any other validation/operations.
    if (local_iov_count == 0 && local_iov_addr.is_null())
        || (remote_iov_count == 0 && remote_iov_addr.is_null())
    {
        return Ok(0);
    }

    let remote_task = current_task.get_task(tid)?;

    current_task.check_ptrace_access_mode(PTRACE_MODE_ATTACH_REALCREDS, &remote_task)?;

    let local_iov = current_task.read_iovec(local_iov_addr, local_iov_count)?;
    let remote_iov = current_task.read_iovec(remote_iov_addr, remote_iov_count)?;
    log_trace!(
        "sys_process_vm_writev(tid={}, local_iov={:?}, remote_iov={:?})",
        tid,
        local_iov,
        remote_iov
    );

    track_stub!(TODO("https://fxbug.dev/322874339"), "process_vm_writev single-copy");
    // NB: According to the man page, this syscall was added to Linux specifically to
    // avoid doing two copies like other IPC mechanisms require. We should avoid this too at some
    // point.
    let mut input = UserBuffersInputBuffer::unified_new(current_task, local_iov)?;
    let remote_mm = remote_task.mm().ok();
    if current_task.has_same_address_space(remote_mm.as_ref()) {
        let mut output = UserBuffersOutputBuffer::unified_new(current_task, remote_iov)?;
        output.write_buffer(&mut input)
    } else {
        let mut output = UserBuffersOutputBuffer::syscall_new(remote_task.deref(), remote_iov)?;
        output.write_buffer(&mut input)
    }
}

pub fn sys_process_mrelease(
    current_task: &CurrentTask,
    pidfd: FdNumber,
    flags: u32,
) -> Result<(), Errno> {
    if flags != 0 {
        return error!(EINVAL);
    }
    let file = current_task.files().get(pidfd)?;
    let task = file.as_pid()?.get_task()?;
    if !task.load_stopped().is_stopped() {
        return error!(EINVAL);
    }

    task.mm()?.mrelease()
}

pub fn sys_membarrier(
    current_task: &CurrentTask,
    cmd: uapi::membarrier_cmd,
    _flags: u32,
    _cpu_id: i32,
) -> Result<u32, Errno> {
    match cmd {
        // This command returns a bit mask of all supported commands.
        // We support everything except for the RSEQ family.
        uapi::membarrier_cmd_MEMBARRIER_CMD_QUERY => Ok(uapi::membarrier_cmd_MEMBARRIER_CMD_GLOBAL
            | uapi::membarrier_cmd_MEMBARRIER_CMD_GLOBAL_EXPEDITED
            | uapi::membarrier_cmd_MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED
            | uapi::membarrier_cmd_MEMBARRIER_CMD_PRIVATE_EXPEDITED
            | uapi::membarrier_cmd_MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED
            | uapi::membarrier_cmd_MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE
            | uapi::membarrier_cmd_MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE),
        // Global and global expedited barriers are treated identically. We don't track
        // registration for global expedited barriers currently.
        uapi::membarrier_cmd_MEMBARRIER_CMD_GLOBAL
        | uapi::membarrier_cmd_MEMBARRIER_CMD_GLOBAL_EXPEDITED => {
            system_barrier(BarrierType::DataMemory);
            Ok(0)
        }
        // Global registration commands are ignored.
        uapi::membarrier_cmd_MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED => Ok(0),
        uapi::membarrier_cmd_MEMBARRIER_CMD_PRIVATE_EXPEDITED => {
            // A private expedited barrier is only issued if the address space is registered
            // for these barriers.
            if current_task.mm()?.membarrier_private_expedited_registered(MembarrierType::Memory) {
                // If a barrier is requested, issue a global barrier.
                system_barrier(BarrierType::DataMemory);
                Ok(0)
            } else {
                error!(EPERM)
            }
        }
        // Private sync core barriers are treated as global instruction stream barriers.
        uapi::membarrier_cmd_MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE => {
            if current_task.mm()?.membarrier_private_expedited_registered(MembarrierType::SyncCore)
            {
                system_barrier(BarrierType::InstructionStream);
                Ok(0)
            } else {
                error!(EPERM)
            }
        }
        uapi::membarrier_cmd_MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED => {
            let _ =
                current_task.mm()?.register_membarrier_private_expedited(MembarrierType::Memory)?;
            Ok(0)
        }

        uapi::membarrier_cmd_MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE => {
            let _ = current_task
                .mm()?
                .register_membarrier_private_expedited(MembarrierType::SyncCore)?;
            Ok(0)
        }
        uapi::membarrier_cmd_MEMBARRIER_CMD_PRIVATE_EXPEDITED_RSEQ => {
            track_stub!(TODO("https://fxbug.dev/447158570"), "membarrier rseq");
            error!(ENOSYS)
        }
        uapi::membarrier_cmd_MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_RSEQ => {
            track_stub!(TODO("https://fxbug.dev/447158570"), "membarrier rseq");
            error!(ENOSYS)
        }
        _ => error!(EINVAL),
    }
}

pub fn sys_futex(
    current_task: &mut CurrentTask,
    addr: UserAddress,
    op: u32,
    value: u32,
    timeout_or_value2: SyscallArg,
    addr2: UserAddress,
    value3: u32,
) -> Result<usize, Errno> {
    if op & FUTEX_PRIVATE_FLAG != 0 {
        do_futex::<PrivateFutexKey>(current_task, addr, op, value, timeout_or_value2, addr2, value3)
    } else {
        do_futex::<SharedFutexKey>(current_task, addr, op, value, timeout_or_value2, addr2, value3)
    }
}

fn do_futex<Key: FutexKey>(
    current_task: &mut CurrentTask,
    addr: UserAddress,
    op: u32,
    value: u32,
    timeout_or_value2: SyscallArg,
    addr2: UserAddress,
    value3: u32,
) -> Result<usize, Errno> {
    let futexes = Key::get_table_from_task(current_task)?;
    let cmd = op & (FUTEX_CMD_MASK as u32);

    let is_realtime = match (cmd, op & FUTEX_CLOCK_REALTIME != 0) {
        // This option bit can be employed only with the FUTEX_WAIT_BITSET, FUTEX_WAIT_REQUEUE_PI,
        // (since Linux 4.5) FUTEX_WAIT, and (since Linux 5.14) FUTEX_LOCK_PI2 operations.
        (FUTEX_WAIT_BITSET | FUTEX_WAIT_REQUEUE_PI | FUTEX_WAIT | FUTEX_LOCK_PI2, true) => true,
        (_, true) => return error!(EINVAL),

        // FUTEX_LOCK_PI always uses realtime.
        (FUTEX_LOCK_PI, _) => true,

        (_, false) => false,
    };

    // The timeout is interpreted differently by WAIT and WAIT_BITSET: WAIT takes a
    // timeout and WAIT_BITSET takes a deadline.
    let read_timespec = |current_task: &CurrentTask| {
        let utime = TimeSpecPtr::new(current_task, timeout_or_value2);
        if utime.is_null() {
            Ok(timespec_from_time(zx::MonotonicInstant::INFINITE))
        } else {
            current_task.read_multi_arch_object(utime)
        }
    };
    let read_timeout = |current_task: &CurrentTask| {
        let timespec = read_timespec(current_task)?;
        let timeout = duration_from_timespec(timespec);
        let deadline = zx::MonotonicInstant::after(timeout?);
        if is_realtime {
            // Since this is a timeout, waiting on the monotonic timeline before it's paused is
            // just as good as actually estimating UTC here.
            track_stub!(TODO("https://fxbug.dev/356912301"), "FUTEX_CLOCK_REALTIME timeout");
        }
        Ok(deadline)
    };
    let read_deadline = |current_task: &CurrentTask| {
        let timespec = read_timespec(current_task)?;
        if is_realtime {
            Ok(TargetTime::RealTime(time_from_timespec::<UtcTimeline>(timespec)?))
        } else {
            Ok(TargetTime::Monotonic(time_from_timespec::<zx::MonotonicTimeline>(timespec)?))
        }
    };

    match cmd {
        FUTEX_WAIT => {
            let deadline = read_timeout(current_task)?;
            let bitset = FUTEX_BITSET_MATCH_ANY;
            do_futex_wait_with_restart::<Key>(
                current_task,
                addr,
                value,
                bitset,
                TargetTime::Monotonic(deadline),
            )?;
            Ok(0)
        }
        FUTEX_WAKE => futexes.wake(current_task, addr, value as usize, FUTEX_BITSET_MATCH_ANY),
        FUTEX_WAKE_OP => {
            track_stub!(TODO("https://fxbug.dev/361181940"), "FUTEX_WAKE_OP");
            error!(ENOSYS)
        }
        FUTEX_WAIT_BITSET => {
            if value3 == 0 {
                return error!(EINVAL);
            }
            let deadline = read_deadline(current_task)?;
            do_futex_wait_with_restart::<Key>(current_task, addr, value, value3, deadline)?;
            Ok(0)
        }
        FUTEX_WAKE_BITSET => {
            if value3 == 0 {
                return error!(EINVAL);
            }
            futexes.wake(current_task, addr, value as usize, value3)
        }
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            let wake_count = value as usize;
            let requeue_count: usize = timeout_or_value2.into();
            if wake_count > i32::MAX as usize || requeue_count > i32::MAX as usize {
                return error!(EINVAL);
            }
            let expected_value = if cmd == FUTEX_CMP_REQUEUE { Some(value3) } else { None };
            futexes.requeue(current_task, addr, wake_count, requeue_count, addr2, expected_value)
        }
        FUTEX_WAIT_REQUEUE_PI => {
            track_stub!(TODO("https://fxbug.dev/361181558"), "FUTEX_WAIT_REQUEUE_PI");
            error!(ENOSYS)
        }
        FUTEX_CMP_REQUEUE_PI => {
            track_stub!(TODO("https://fxbug.dev/361181773"), "FUTEX_CMP_REQUEUE_PI");
            error!(ENOSYS)
        }
        FUTEX_LOCK_PI | FUTEX_LOCK_PI2 => {
            futexes.lock_pi(current_task, addr, read_timeout(current_task)?)?;
            Ok(0)
        }
        FUTEX_TRYLOCK_PI => {
            track_stub!(TODO("https://fxbug.dev/361175318"), "FUTEX_TRYLOCK_PI");
            error!(ENOSYS)
        }
        FUTEX_UNLOCK_PI => {
            futexes.unlock_pi(current_task, addr)?;
            Ok(0)
        }
        _ => {
            track_stub!(TODO("https://fxbug.dev/322875124"), "futex unknown command", cmd);
            error!(ENOSYS)
        }
    }
}

fn do_futex_wait_with_restart<Key: FutexKey>(
    current_task: &mut CurrentTask,
    addr: UserAddress,
    value: u32,
    mask: u32,
    deadline: TargetTime,
) -> Result<(), Errno> {
    let futexes = Key::get_table_from_task(current_task)?;
    let result = match deadline {
        TargetTime::Monotonic(mono_deadline) => {
            futexes.wait(current_task, addr, value, mask, mono_deadline)
        }
        TargetTime::BootInstant(boot_deadline) => {
            let timer_slack = current_task.read().get_timerslack();
            futexes.wait_boot(current_task, addr, value, mask, boot_deadline, timer_slack)
        }
        TargetTime::RealTime(utc_deadline) => {
            // We convert real time deadlines to boot time deadlines since we cannot wait using a UTC deadline.
            let (boot_deadline, _) = estimate_boot_deadline_from_utc(utc_deadline);
            let timer_slack = current_task.read().get_timerslack();
            futexes.wait_boot(current_task, addr, value, mask, boot_deadline, timer_slack)
        }
    };
    match result {
        Err(err) if err == EINTR => {
            current_task.set_syscall_restart_func(move |current_task| {
                do_futex_wait_with_restart::<Key>(current_task, addr, value, mask, deadline)
            });
            error!(ERESTART_RESTARTBLOCK)
        }
        result => result,
    }
}

pub fn sys_get_robust_list(
    current_task: &CurrentTask,
    tid: tid_t,
    user_head_ptr: UserRef<UserAddress>,
    user_len_ptr: UserRef<usize>,
) -> Result<(), Errno> {
    if tid < 0 {
        return error!(EINVAL);
    }
    if user_head_ptr.is_null() || user_len_ptr.is_null() {
        return error!(EFAULT);
    }
    let task = if tid == 0 {
        current_task.task.clone()
    } else {
        let task = current_task.get_task(tid)?;
        current_task.check_ptrace_access_mode(PTRACE_MODE_READ_REALCREDS, &task)?;
        task
    };
    current_task.write_object(user_head_ptr, &task.read().robust_list_head.addr())?;
    current_task.write_object(user_len_ptr, &std::mem::size_of::<robust_list_head>())?;
    Ok(())
}

pub fn sys_set_robust_list(
    current_task: &CurrentTask,
    user_head: UserRef<robust_list_head>,
    len: usize,
) -> Result<(), Errno> {
    if len != std::mem::size_of::<robust_list_head>() {
        return error!(EINVAL);
    }
    current_task.write().robust_list_head = user_head.into();
    Ok(())
}

pub fn sys_mlock(
    current_task: &CurrentTask,
    addr: UserAddress,
    length: usize,
) -> Result<(), Errno> {
    // If flags is 0, mlock2() behaves exactly the same as mlock().
    sys_mlock2(current_task, addr, length, 0)
}

pub fn sys_mlock2(
    current_task: &CurrentTask,
    addr: UserAddress,
    length: usize,
    flags: u64,
) -> Result<(), Errno> {
    const KNOWN_FLAGS: u64 = MLOCK_ONFAULT as u64;
    if (flags & !KNOWN_FLAGS) != 0 {
        return error!(EINVAL);
    }
    let on_fault = flags & MLOCK_ONFAULT as u64 != 0;
    current_task.mm()?.mlock(current_task, addr, length, on_fault)
}

pub fn sys_munlock(
    current_task: &CurrentTask,
    addr: UserAddress,
    length: usize,
) -> Result<(), Errno> {
    current_task.mm()?.munlock(current_task, addr, length)
}

pub fn sys_mlockall(_current_task: &CurrentTask, _flags: u64) -> Result<(), Errno> {
    track_stub!(TODO("https://fxbug.dev/297292097"), "mlockall()");
    error!(ENOSYS)
}

pub fn sys_munlockall(_current_task: &CurrentTask, _flags: u64) -> Result<(), Errno> {
    track_stub!(TODO("https://fxbug.dev/297292097"), "munlockall()");
    error!(ENOSYS)
}

pub fn sys_mincore(
    _current_task: &CurrentTask,
    _addr: UserAddress,
    _length: usize,
    _out: UserRef<u8>,
) -> Result<(), Errno> {
    track_stub!(TODO("https://fxbug.dev/297372240"), "mincore()");
    error!(ENOSYS)
}

// Syscalls for arch32 usage
#[cfg(target_arch = "aarch64")]
mod arch32 {
    use crate::mm::PAGE_SIZE;
    use crate::mm::memory_accessor::MemoryAccessorExt;
    use crate::mm::syscalls::{UserAddress, sys_mmap};
    use crate::task::{CurrentTask, RobustListHeadPtr};
    use crate::vfs::FdNumber;
    use starnix_uapi::auth::PTRACE_MODE_READ_REALCREDS;
    use starnix_uapi::errors::Errno;
    use starnix_uapi::user_address::UserRef;
    use starnix_uapi::{error, uapi};

    pub fn sys_arch32_set_robust_list(
        current_task: &CurrentTask,
        user_head: UserRef<uapi::arch32::robust_list_head>,
        len: usize,
    ) -> Result<(), Errno> {
        if len != std::mem::size_of::<uapi::arch32::robust_list_head>() {
            return error!(EINVAL);
        }
        current_task.write().robust_list_head = RobustListHeadPtr::from_32(user_head);
        Ok(())
    }

    pub fn sys_arch32_get_robust_list(
        current_task: &CurrentTask,
        tid: starnix_uapi::tid_t,
        user_head_ptr: UserRef<u32>,
        user_len_ptr: UserRef<u32>,
    ) -> Result<(), Errno> {
        if tid < 0 {
            return error!(EINVAL);
        }
        if user_head_ptr.is_null() || user_len_ptr.is_null() {
            return error!(EFAULT);
        }
        let task = if tid == 0 {
            current_task.task.clone()
        } else {
            let task = current_task.get_task(tid)?;
            current_task.check_ptrace_access_mode(PTRACE_MODE_READ_REALCREDS, &task)?;
            task
        };

        let addr = task.read().robust_list_head.addr().ptr() as u32;
        current_task.write_object(user_head_ptr, &addr)?;
        current_task.write_object(
            user_len_ptr,
            &(std::mem::size_of::<uapi::arch32::robust_list_head>() as u32),
        )?;
        Ok(())
    }

    pub fn sys_arch32_mmap2(
        current_task: &mut CurrentTask,
        addr: UserAddress,
        length: usize,
        prot: u32,
        flags: u32,
        fd: FdNumber,
        offset: u64,
    ) -> Result<UserAddress, Errno> {
        sys_mmap(current_task, addr, length, prot, flags, fd, offset * *PAGE_SIZE)
    }

    pub fn sys_arch32_munmap(
        current_task: &CurrentTask,
        addr: UserAddress,
        length: usize,
    ) -> Result<(), Errno> {
        if !addr.is_lower_32bit() || length >= (1 << 32) {
            return error!(EINVAL);
        }
        current_task.mm()?.unmap(addr, length)?;
        Ok(())
    }

    pub use super::{
        sys_futex as sys_arch32_futex, sys_madvise as sys_arch32_madvise,
        sys_membarrier as sys_arch32_membarrier, sys_mincore as sys_arch32_mincore,
        sys_mlock as sys_arch32_mlock, sys_mlock2 as sys_arch32_mlock2,
        sys_mlockall as sys_arch32_mlockall, sys_mremap as sys_arch32_mremap,
        sys_msync as sys_arch32_msync, sys_munlock as sys_arch32_munlock,
        sys_munlockall as sys_arch32_munlockall,
        sys_process_mrelease as sys_arch32_process_mrelease,
        sys_process_vm_readv as sys_arch32_process_vm_readv,
    };
}

#[cfg(target_arch = "aarch64")]
pub use arch32::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mm::memory::MemoryObject;
    use crate::testing::*;
    use starnix_uapi::errors::EEXIST;
    use starnix_uapi::{MREMAP_FIXED, MREMAP_MAYMOVE, PROT_READ};

    #[::fuchsia::test]
    async fn test_mmap_with_colliding_hint() {
        spawn_kernel_and_run(async |current_task| {
            let page_size = *PAGE_SIZE;

            let mapped_address = map_memory(&current_task, UserAddress::default(), page_size);
            match do_mmap(
                &current_task,
                mapped_address,
                page_size as usize,
                PROT_READ,
                MAP_PRIVATE | MAP_ANONYMOUS,
                FdNumber::from_raw(-1),
                0,
            ) {
                Ok(address) => {
                    assert_ne!(address, mapped_address);
                }
                error => {
                    panic!("mmap with colliding hint failed: {error:?}");
                }
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_mmap_with_fixed_collision() {
        spawn_kernel_and_run(async |current_task| {
            let page_size = *PAGE_SIZE;

            let mapped_address = map_memory(&current_task, UserAddress::default(), page_size);
            match do_mmap(
                &current_task,
                mapped_address,
                page_size as usize,
                PROT_READ,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
                FdNumber::from_raw(-1),
                0,
            ) {
                Ok(address) => {
                    assert_eq!(address, mapped_address);
                }
                error => {
                    panic!("mmap with fixed collision failed: {error:?}");
                }
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_mmap_with_fixed_noreplace_collision() {
        spawn_kernel_and_run(async |current_task| {
            let page_size = *PAGE_SIZE;

            let mapped_address = map_memory(&current_task, UserAddress::default(), page_size);
            match do_mmap(
                &current_task,
                mapped_address,
                page_size as usize,
                PROT_READ,
                MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE,
                FdNumber::from_raw(-1),
                0,
            ) {
                Err(errno) => {
                    assert_eq!(errno, EEXIST);
                }
                result => {
                    panic!("mmap with fixed_noreplace collision failed: {result:?}");
                }
            }
        })
        .await;
    }

    /// It is ok to call munmap with an address that is a multiple of the page size, and
    /// a non-zero length.
    #[::fuchsia::test]
    async fn test_munmap() {
        spawn_kernel_and_run(async |current_task| {
            let mapped_address = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            assert_eq!(sys_munmap(&current_task, mapped_address, *PAGE_SIZE as usize), Ok(()));

            // Verify that the memory is no longer readable.
            assert_eq!(current_task.read_memory_to_array::<5>(mapped_address), error!(EFAULT));
        })
        .await;
    }

    /// It is ok to call munmap on an unmapped range.
    #[::fuchsia::test]
    async fn test_munmap_not_mapped() {
        spawn_kernel_and_run(async |current_task| {
            let mapped_address = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            assert_eq!(sys_munmap(&current_task, mapped_address, *PAGE_SIZE as usize), Ok(()));
            assert_eq!(sys_munmap(&current_task, mapped_address, *PAGE_SIZE as usize), Ok(()));
        })
        .await;
    }

    /// It is an error to call munmap with a length of 0.
    #[::fuchsia::test]
    async fn test_munmap_0_length() {
        spawn_kernel_and_run(async |current_task| {
            let mapped_address = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            assert_eq!(sys_munmap(&current_task, mapped_address, 0), error!(EINVAL));
        })
        .await;
    }

    /// It is an error to call munmap with an address that is not a multiple of the page size.
    #[::fuchsia::test]
    async fn test_munmap_not_aligned() {
        spawn_kernel_and_run(async |current_task| {
            let mapped_address = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            assert_eq!(
                sys_munmap(&current_task, (mapped_address + 1u64).unwrap(), *PAGE_SIZE as usize),
                error!(EINVAL)
            );

            // Verify that the memory is still readable.
            assert!(current_task.read_memory_to_array::<5>(mapped_address).is_ok());
        })
        .await;
    }

    /// The entire page should be unmapped, not just the range [address, address + length).
    #[::fuchsia::test]
    async fn test_munmap_unmap_partial() {
        spawn_kernel_and_run(async |current_task| {
            let mapped_address = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            assert_eq!(
                sys_munmap(&current_task, mapped_address, (*PAGE_SIZE as usize) / 2),
                Ok(())
            );

            // Verify that memory can't be read in either half of the page.
            assert_eq!(current_task.read_memory_to_array::<5>(mapped_address), error!(EFAULT));
            assert_eq!(
                current_task
                    .read_memory_to_array::<5>((mapped_address + (*PAGE_SIZE - 2)).unwrap()),
                error!(EFAULT)
            );
        })
        .await;
    }

    /// All pages that intersect the munmap range should be unmapped.
    #[::fuchsia::test]
    async fn test_munmap_multiple_pages() {
        spawn_kernel_and_run(async |current_task| {
            let mapped_address = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 2);
            assert_eq!(
                sys_munmap(&current_task, mapped_address, (*PAGE_SIZE as usize) + 1),
                Ok(())
            );

            // Verify that neither page is readable.
            assert_eq!(current_task.read_memory_to_array::<5>(mapped_address), error!(EFAULT));
            assert_eq!(
                current_task
                    .read_memory_to_array::<5>((mapped_address + (*PAGE_SIZE + 1u64)).unwrap()),
                error!(EFAULT)
            );
        })
        .await;
    }

    /// Only the pages that intersect the munmap range should be unmapped.
    #[::fuchsia::test]
    async fn test_munmap_one_of_many_pages() {
        spawn_kernel_and_run(async |current_task| {
            let mapped_address = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 2);
            assert_eq!(
                sys_munmap(&current_task, mapped_address, (*PAGE_SIZE as usize) - 1),
                Ok(())
            );

            // Verify that the second page is still readable.
            assert_eq!(current_task.read_memory_to_array::<5>(mapped_address), error!(EFAULT));
            assert!(
                current_task
                    .read_memory_to_array::<5>((mapped_address + (*PAGE_SIZE + 1u64)).unwrap())
                    .is_ok()
            );
        })
        .await;
    }

    /// Unmap the middle page of a mapping.
    #[::fuchsia::test]
    async fn test_munmap_middle_page() {
        spawn_kernel_and_run(async |current_task| {
            let mapped_address = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 3);
            assert_eq!(
                sys_munmap(
                    &current_task,
                    (mapped_address + *PAGE_SIZE).unwrap(),
                    *PAGE_SIZE as usize
                ),
                Ok(())
            );

            // Verify that the first and third pages are still readable.
            assert!(current_task.read_memory_to_vec(mapped_address, 5).is_ok());
            assert_eq!(
                current_task.read_memory_to_vec((mapped_address + *PAGE_SIZE).unwrap(), 5),
                error!(EFAULT)
            );
            assert!(
                current_task
                    .read_memory_to_vec((mapped_address + (*PAGE_SIZE * 2)).unwrap(), 5)
                    .is_ok()
            );
        })
        .await;
    }

    /// Unmap a range of pages that includes disjoint mappings.
    #[::fuchsia::test]
    async fn test_munmap_many_mappings() {
        spawn_kernel_and_run(async |current_task| {
            let mapped_addresses: Vec<_> = std::iter::repeat_with(|| {
                map_memory(&current_task, UserAddress::default(), *PAGE_SIZE)
            })
            .take(3)
            .collect();
            let min_address = *mapped_addresses.iter().min().unwrap();
            let max_address = *mapped_addresses.iter().max().unwrap();
            let unmap_length = (max_address - min_address) + *PAGE_SIZE as usize;

            assert_eq!(sys_munmap(&current_task, min_address, unmap_length), Ok(()));

            // Verify that none of the mapped pages are readable.
            for mapped_address in mapped_addresses {
                assert_eq!(current_task.read_memory_to_vec(mapped_address, 5), error!(EFAULT));
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_msync_validates_address_range() {
        spawn_kernel_and_run(async |current_task| {
            // Map 3 pages and test that ranges covering these pages return no error.
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 3);
            assert_eq!(sys_msync(&current_task, addr, *PAGE_SIZE as usize * 3, 0), Ok(()));
            assert_eq!(sys_msync(&current_task, addr, *PAGE_SIZE as usize * 2, 0), Ok(()));
            assert_eq!(
                sys_msync(&current_task, (addr + *PAGE_SIZE).unwrap(), *PAGE_SIZE as usize * 2, 0),
                Ok(())
            );

            // Unmap the middle page and test that ranges covering that page return ENOMEM.
            sys_munmap(&current_task, (addr + *PAGE_SIZE).unwrap(), *PAGE_SIZE as usize)
                .expect("unmap middle");
            assert_eq!(sys_msync(&current_task, addr, *PAGE_SIZE as usize, 0), Ok(()));
            assert_eq!(
                sys_msync(&current_task, addr, *PAGE_SIZE as usize * 3, starnix_uapi::MS_SYNC),
                error!(ENOMEM)
            );
            assert_eq!(
                sys_msync(&current_task, addr, *PAGE_SIZE as usize * 2, starnix_uapi::MS_SYNC),
                error!(ENOMEM)
            );
            assert_eq!(
                sys_msync(
                    &current_task,
                    (addr + *PAGE_SIZE).unwrap(),
                    *PAGE_SIZE as usize * 2,
                    starnix_uapi::MS_SYNC
                ),
                error!(ENOMEM)
            );
            assert_eq!(
                sys_msync(
                    &current_task,
                    (addr + (*PAGE_SIZE * 2)).unwrap(),
                    *PAGE_SIZE as usize,
                    0
                ),
                Ok(())
            );

            // Map the middle page back and test that ranges covering the three pages
            // (spanning multiple ranges) return no error.
            assert_eq!(
                map_memory(&current_task, (addr + *PAGE_SIZE).unwrap(), *PAGE_SIZE),
                (addr + *PAGE_SIZE).unwrap()
            );
            assert_eq!(sys_msync(&current_task, addr, *PAGE_SIZE as usize * 3, 0), Ok(()));
            assert_eq!(sys_msync(&current_task, addr, *PAGE_SIZE as usize * 2, 0), Ok(()));
            assert_eq!(
                sys_msync(&current_task, (addr + *PAGE_SIZE).unwrap(), *PAGE_SIZE as usize * 2, 0),
                Ok(())
            );
        })
        .await;
    }

    /// Shrinks an entire range.
    #[::fuchsia::test]
    async fn test_mremap_shrink_whole_range_from_end() {
        spawn_kernel_and_run(async |current_task| {
            // Map 2 pages.
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 2);
            fill_page(&current_task, addr, 'a');
            fill_page(&current_task, (addr + *PAGE_SIZE).unwrap(), 'b');

            // Shrink the mapping from 2 to 1 pages.
            assert_eq!(
                remap_memory(
                    &current_task,
                    addr,
                    *PAGE_SIZE * 2,
                    *PAGE_SIZE,
                    0,
                    UserAddress::default()
                ),
                Ok(addr)
            );

            check_page_eq(&current_task, addr, 'a');
            check_unmapped(&current_task, (addr + *PAGE_SIZE).unwrap());
        })
        .await;
    }

    /// Shrinks part of a range, introducing a hole in the middle.
    #[::fuchsia::test]
    async fn test_mremap_shrink_partial_range() {
        spawn_kernel_and_run(async |current_task| {
            // Map 3 pages.
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 3);
            fill_page(&current_task, addr, 'a');
            fill_page(&current_task, (addr + *PAGE_SIZE).unwrap(), 'b');
            fill_page(&current_task, (addr + (*PAGE_SIZE * 2)).unwrap(), 'c');

            // Shrink the first 2 pages down to 1, creating a hole.
            assert_eq!(
                remap_memory(
                    &current_task,
                    addr,
                    *PAGE_SIZE * 2,
                    *PAGE_SIZE,
                    0,
                    UserAddress::default()
                ),
                Ok(addr)
            );

            check_page_eq(&current_task, addr, 'a');
            check_unmapped(&current_task, (addr + *PAGE_SIZE).unwrap());
            check_page_eq(&current_task, (addr + (*PAGE_SIZE * 2)).unwrap(), 'c');
        })
        .await;
    }

    /// Shrinking doesn't care if the range specified spans multiple mappings.
    #[::fuchsia::test]
    async fn test_mremap_shrink_across_ranges() {
        spawn_kernel_and_run(async |current_task| {
            // Map 3 pages, unmap the middle, then map the middle again. This will leave us with
            // 3 contiguous mappings.
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 3);
            assert_eq!(
                sys_munmap(&current_task, (addr + *PAGE_SIZE).unwrap(), *PAGE_SIZE as usize),
                Ok(())
            );
            assert_eq!(
                map_memory(&current_task, (addr + *PAGE_SIZE).unwrap(), *PAGE_SIZE),
                (addr + *PAGE_SIZE).unwrap()
            );

            fill_page(&current_task, addr, 'a');
            fill_page(&current_task, (addr + *PAGE_SIZE).unwrap(), 'b');
            fill_page(&current_task, (addr + (*PAGE_SIZE * 2)).unwrap(), 'c');

            // Remap over all three mappings, shrinking to 1 page.
            assert_eq!(
                remap_memory(
                    &current_task,
                    addr,
                    *PAGE_SIZE * 3,
                    *PAGE_SIZE,
                    0,
                    UserAddress::default()
                ),
                Ok(addr)
            );

            check_page_eq(&current_task, addr, 'a');
            check_unmapped(&current_task, (addr + *PAGE_SIZE).unwrap());
            check_unmapped(&current_task, (addr + (*PAGE_SIZE * 2)).unwrap());
        })
        .await;
    }

    /// Grows a mapping in-place.
    #[::fuchsia::test]
    async fn test_mremap_grow_in_place() {
        spawn_kernel_and_run(async |current_task| {
            // Map 3 pages, unmap the middle, leaving a hole.
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 3);
            fill_page(&current_task, addr, 'a');
            fill_page(&current_task, (addr + *PAGE_SIZE).unwrap(), 'b');
            fill_page(&current_task, (addr + (*PAGE_SIZE * 2)).unwrap(), 'c');
            assert_eq!(
                sys_munmap(&current_task, (addr + *PAGE_SIZE).unwrap(), *PAGE_SIZE as usize),
                Ok(())
            );

            // Grow the first page in-place into the middle.
            assert_eq!(
                remap_memory(
                    &current_task,
                    addr,
                    *PAGE_SIZE,
                    *PAGE_SIZE * 2,
                    0,
                    UserAddress::default()
                ),
                Ok(addr)
            );

            check_page_eq(&current_task, addr, 'a');

            // The middle page should be new, and not just pointing to the original middle page filled
            // with 'b'.
            check_page_ne(&current_task, (addr + *PAGE_SIZE).unwrap(), 'b');

            check_page_eq(&current_task, (addr + (*PAGE_SIZE * 2)).unwrap(), 'c');
        })
        .await;
    }

    /// Tries to grow a set of pages that cannot fit, and forces a move.
    #[::fuchsia::test]
    async fn test_mremap_grow_maymove() {
        spawn_kernel_and_run(async |current_task| {
            // Map 3 pages.
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 3);
            fill_page(&current_task, addr, 'a');
            fill_page(&current_task, (addr + *PAGE_SIZE).unwrap(), 'b');
            fill_page(&current_task, (addr + (*PAGE_SIZE * 2)).unwrap(), 'c');

            // Grow the first two pages by 1, forcing a move.
            let new_addr = remap_memory(
                &current_task,
                addr,
                *PAGE_SIZE * 2,
                *PAGE_SIZE * 3,
                MREMAP_MAYMOVE,
                UserAddress::default(),
            )
            .expect("failed to mremap");

            assert_ne!(new_addr, addr, "mremap did not move the mapping");

            // The first two pages should have been moved.
            check_unmapped(&current_task, addr);
            check_unmapped(&current_task, (addr + *PAGE_SIZE).unwrap());

            // The third page should still be present.
            check_page_eq(&current_task, (addr + (*PAGE_SIZE * 2)).unwrap(), 'c');

            // The moved pages should have the same contents.
            check_page_eq(&current_task, new_addr, 'a');
            check_page_eq(&current_task, (new_addr + *PAGE_SIZE).unwrap(), 'b');

            // The newly grown page should not be the same as the original third page.
            check_page_ne(&current_task, (new_addr + (*PAGE_SIZE * 2)).unwrap(), 'c');
        })
        .await;
    }

    /// Shrinks a set of pages and move them to a fixed location.
    #[::fuchsia::test]
    async fn test_mremap_shrink_fixed() {
        spawn_kernel_and_run(async |current_task| {
            // Map 2 pages which will act as the destination.
            let dst_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 2);
            fill_page(&current_task, dst_addr, 'y');
            fill_page(&current_task, (dst_addr + *PAGE_SIZE).unwrap(), 'z');

            // Map 3 pages.
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 3);
            fill_page(&current_task, addr, 'a');
            fill_page(&current_task, (addr + *PAGE_SIZE).unwrap(), 'b');
            fill_page(&current_task, (addr + (*PAGE_SIZE * 2)).unwrap(), 'c');

            // Shrink the first two pages and move them to overwrite the mappings at `dst_addr`.
            let new_addr = remap_memory(
                &current_task,
                addr,
                *PAGE_SIZE * 2,
                *PAGE_SIZE,
                MREMAP_MAYMOVE | MREMAP_FIXED,
                dst_addr,
            )
            .expect("failed to mremap");

            assert_eq!(new_addr, dst_addr, "mremap did not move the mapping");

            // The first two pages should have been moved.
            check_unmapped(&current_task, addr);
            check_unmapped(&current_task, (addr + *PAGE_SIZE).unwrap());

            // The third page should still be present.
            check_page_eq(&current_task, (addr + (*PAGE_SIZE * 2)).unwrap(), 'c');

            // The first moved page should have the same contents.
            check_page_eq(&current_task, new_addr, 'a');

            // The second page should be part of the original dst mapping.
            check_page_eq(&current_task, (new_addr + *PAGE_SIZE).unwrap(), 'z');
        })
        .await;
    }

    /// Clobbers the middle of an existing mapping with mremap to a fixed location.
    #[::fuchsia::test]
    async fn test_mremap_clobber_memory_mapping() {
        spawn_kernel_and_run(async |current_task| {
            let dst_memory = MemoryObject::from(zx::Vmo::create(2 * *PAGE_SIZE).unwrap());
            dst_memory.write(&['x' as u8].repeat(*PAGE_SIZE as usize), 0).unwrap();
            dst_memory.write(&['y' as u8].repeat(*PAGE_SIZE as usize), *PAGE_SIZE).unwrap();

            let dst_addr = current_task
                .mm()
                .unwrap()
                .map_memory(
                    DesiredAddress::Any,
                    dst_memory.into(),
                    0,
                    2 * (*PAGE_SIZE as usize),
                    ProtectionFlags::READ,
                    MappingOptions::empty(),
                    MappingName::None,
                )
                .unwrap();

            // Map 3 pages.
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE * 3);
            fill_page(&current_task, addr, 'a');
            fill_page(&current_task, (addr + *PAGE_SIZE).unwrap(), 'b');
            fill_page(&current_task, (addr + (*PAGE_SIZE * 2)).unwrap(), 'c');

            // Overwrite the second page of the mapping with the second page of the anonymous mapping.
            let remapped_addr = sys_mremap(
                &*current_task,
                (addr + *PAGE_SIZE).unwrap(),
                *PAGE_SIZE as usize,
                *PAGE_SIZE as usize,
                MREMAP_FIXED | MREMAP_MAYMOVE,
                (dst_addr + *PAGE_SIZE).unwrap(),
            )
            .unwrap();

            assert_eq!(remapped_addr, (dst_addr + *PAGE_SIZE).unwrap());

            check_page_eq(&current_task, addr, 'a');
            check_unmapped(&current_task, (addr + *PAGE_SIZE).unwrap());
            check_page_eq(&current_task, (addr + (2 * *PAGE_SIZE)).unwrap(), 'c');

            check_page_eq(&current_task, dst_addr, 'x');
            check_page_eq(&current_task, (dst_addr + *PAGE_SIZE).unwrap(), 'b');
        })
        .await;
    }

    #[cfg(target_arch = "x86_64")]
    #[::fuchsia::test]
    async fn test_map_32_bit() {
        use starnix_uapi::PROT_WRITE;

        spawn_kernel_and_run(async |current_task| {
            let page_size = *PAGE_SIZE;

            for _i in 0..256 {
                match do_mmap(
                    &current_task,
                    UserAddress::from(0),
                    page_size as usize,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS | MAP_32BIT,
                    FdNumber::from_raw(-1),
                    0,
                ) {
                    Ok(address) => {
                        let memory_end = address.ptr() + page_size as usize;
                        assert!(memory_end <= 0x80000000);
                    }
                    error => {
                        panic!("mmap with MAP_32BIT failed: {error:?}");
                    }
                }
            }
        })
        .await;
    }
}
