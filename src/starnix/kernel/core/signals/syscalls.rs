// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use super::signal_handling::sys_restart_syscall;
use super::signalfd::SignalFd;
use crate::mm::MemoryAccessorExt;
use crate::security;
use crate::signals::{
    IntoSignalInfoOptions, SignalDetail, SignalInfo, UncheckedSignalInfo,
    restore_from_signal_handler, send_signal,
};
use crate::task::{
    CurrentTask, Pid, ProcessEntryRef, ProcessSelector, RunState, Task, TaskMutableState,
    ThreadGroup, ThreadGroupLifecycleWaitValue, WaitResult, WaitableChildResult, Waiter,
};
use crate::vfs::{FdFlags, FdNumber};
use fuchsia_rcu::RcuReadScope;
use starnix_uapi::user_address::{ArchSpecific, MultiArchUserRef};
use starnix_uapi::{tid_t, uapi};

use starnix_logging::track_stub;
use starnix_sync::{InterruptibleEvent, WakeReason};
use starnix_syscalls::SyscallResult;
use starnix_types::time::{duration_from_timespec, timeval_from_duration};
use starnix_uapi::errors::{EINTR, ETIMEDOUT, Errno, ErrnoResultExt};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::signals::{SigSet, Signal, UNBLOCKABLE_SIGNALS, UncheckedSignal};
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::{
    __WALL, __WCLONE, __WNOTHREAD, P_ALL, P_PGID, P_PID, P_PIDFD, SFD_CLOEXEC, SFD_NONBLOCK,
    SI_TKILL, SIG_BLOCK, SIG_SETMASK, SIG_UNBLOCK, SS_AUTODISARM, SS_DISABLE, SS_ONSTACK,
    WCONTINUED, WEXITED, WNOHANG, WNOWAIT, WSTOPPED, WUNTRACED, errno, error, pid_t, rusage,
    sigaltstack,
};
use static_assertions::const_assert_eq;
use zerocopy::{FromBytes, Immutable, IntoBytes};

pub type RUsagePtr = MultiArchUserRef<uapi::rusage, uapi::arch32::rusage>;
type SigAction64Ptr = MultiArchUserRef<uapi::sigaction_t, uapi::arch32::sigaction64_t>;
type SigActionPtr = MultiArchUserRef<uapi::sigaction_t, uapi::arch32::sigaction_t>;

/// The `rt_sigaction` syscall allows the calling process to examine and change the action
/// associated with a specific signal.
///
/// # Args
/// * `signum`: The signal number to examine or change. It can be any valid signal except
///   `SIGKILL` and `SIGSTOP`.
/// * `user_action`: A pointer to a `sigaction` structure. If it is not null, the new action
///   for signal `signum` is installed from it.
/// * `user_old_action`: A pointer to a `sigaction` structure. If it is not null, the previous
///   action is saved in it.
/// * `sigset_size`: The size in bytes of the signal sets in `user_action` and `user_old_action`.
///
/// # Returns
/// `Ok(())` on success. Otherwise, returns an `Errno` with the error code.
pub fn sys_rt_sigaction(
    current_task: &CurrentTask,
    signum: UncheckedSignal,
    user_action: SigAction64Ptr,
    user_old_action: SigAction64Ptr,
    sigset_size: usize,
) -> Result<(), Errno> {
    if user_action.is_arch32() && sigset_size == std::mem::size_of::<uapi::arch32::sigset_t>() {
        let user_action = SigActionPtr::from_32(user_action.addr().into());
        let user_old_action = SigActionPtr::from_32(user_old_action.addr().into());
        return rt_sigaction(current_task, signum, user_action, user_old_action);
    }

    if sigset_size != std::mem::size_of::<uapi::sigset_t>() {
        return error!(EINVAL);
    }
    rt_sigaction(current_task, signum, user_action, user_old_action)
}

fn rt_sigaction<Arch32SigAction>(
    current_task: &CurrentTask,
    signum: UncheckedSignal,
    user_action: MultiArchUserRef<uapi::sigaction_t, Arch32SigAction>,
    user_old_action: MultiArchUserRef<uapi::sigaction_t, Arch32SigAction>,
) -> Result<(), Errno>
where
    Arch32SigAction:
        IntoBytes + FromBytes + Immutable + TryFrom<uapi::sigaction_t> + TryInto<uapi::sigaction_t>,
{
    let signal = Signal::try_from(signum)?;

    let new_signal_action = if !user_action.is_null() {
        // Actions can't be set for SIGKILL and SIGSTOP, but the actions for these signals can
        // still be returned in `user_old_action`, so only return early if the intention is to
        // set an action (i.e., the user_action is non-null).
        if signal.is_unblockable() {
            return error!(EINVAL);
        }

        let signal_action = current_task.read_multi_arch_object(user_action)?;
        Some(signal_action)
    } else {
        None
    };

    let signal_actions = &current_task.thread_group().signal_actions;
    let old_action = if let Some(new_signal_action) = new_signal_action {
        signal_actions.set(signal, new_signal_action)
    } else {
        signal_actions.get(signal)
    };

    if !user_old_action.is_null() {
        current_task.write_multi_arch_object(user_old_action, old_action)?;
    }

    Ok(())
}

/// The `rt_sigpending` syscall returns the set of signals that are pending for delivery to the
/// calling thread.
///
/// # Args
/// * `set`: A pointer to a `sigset_t` where the set of pending signals is stored.
/// * `sigset_size`: The size of the signal set, in bytes.
///
/// # Returns
/// `Ok(())` on success. Otherwise, returns an `Errno` with the error code.
pub fn sys_rt_sigpending(
    current_task: &CurrentTask,
    set: UserRef<SigSet>,
    sigset_size: usize,
) -> Result<(), Errno> {
    if sigset_size != std::mem::size_of::<SigSet>() {
        return error!(EINVAL);
    }

    let signals = current_task.read().pending_signals();
    current_task.write_object(set, &signals)?;
    Ok(())
}

/// The `rt_sigprocmask` syscall is used to fetch and/or change the signal mask of the calling
/// thread.
///
/// # Args
/// * `how`: Specifies how the signal mask should be changed. Can be `SIG_BLOCK`, `SIG_UNBLOCK`,
///   or `SIG_SETMASK`.
/// * `user_set`: A pointer to a signal set. The interpretation of this set depends on `how`.
/// * `user_old_set`: If not null, the previous signal mask is stored here.
/// * `sigset_size`: The size of the signal set, in bytes.
///
/// # Returns
/// `Ok(())` on success. Otherwise, returns an `Errno` with the error code.
pub fn sys_rt_sigprocmask(
    current_task: &CurrentTask,
    how: u32,
    user_set: UserRef<SigSet>,
    user_old_set: UserRef<SigSet>,
    sigset_size: usize,
) -> Result<(), Errno> {
    if sigset_size != std::mem::size_of::<SigSet>() {
        return error!(EINVAL);
    }
    match how {
        SIG_BLOCK | SIG_UNBLOCK | SIG_SETMASK => (),
        _ => return error!(EINVAL),
    };

    // Read the new mask. This must be done before the old mask is written to `user_old_set`
    // since it might point to the same location as `user_set`.
    let mut new_mask = SigSet::default();
    if !user_set.is_null() {
        new_mask = current_task.read_object(user_set)?;
    }

    let mut state = current_task.write();
    let signal_mask = state.signal_mask();
    // If old_set is not null, store the previous value in old_set.
    if !user_old_set.is_null() {
        current_task.write_object(user_old_set, &signal_mask)?;
    }

    // If set is null, how is ignored and the mask is not updated.
    if user_set.is_null() {
        return Ok(());
    }

    let signal_mask = match how {
        SIG_BLOCK => signal_mask | new_mask,
        SIG_UNBLOCK => signal_mask & !new_mask,
        SIG_SETMASK => new_mask,
        // Arguments have already been verified, this should never match.
        _ => return error!(EINVAL),
    };
    state.set_signal_mask(signal_mask);

    Ok(())
}

type SigAltStackPtr = MultiArchUserRef<uapi::sigaltstack, uapi::arch32::sigaltstack>;

/// The `sigaltstack` syscall allows a process to define an alternate signal stack.
///
/// # Args
/// * `user_ss`: A pointer to a `sigaltstack` structure specifying the new alternate signal stack.
/// * `user_old_ss`: If not null, the previous alternate signal stack is stored here.
///
/// # Returns
/// `Ok(())` on success. Otherwise, returns an `Errno` with the error code.
pub fn sys_sigaltstack(
    current_task: &CurrentTask,
    user_ss: SigAltStackPtr,
    user_old_ss: SigAltStackPtr,
) -> Result<(), Errno> {
    let stack_pointer_register = current_task.thread_state.registers.stack_pointer_register();
    let mut state = current_task.write();
    let on_signal_stack = state.on_signal_stack(stack_pointer_register);

    let mut ss = sigaltstack::default();
    if !user_ss.is_null() {
        if on_signal_stack {
            return error!(EPERM);
        }
        ss = current_task.read_multi_arch_object(user_ss)?;
        if (ss.ss_flags & !((SS_AUTODISARM | SS_DISABLE) as i32)) != 0 {
            return error!(EINVAL);
        }
        let min_stack_size =
            if current_task.is_arch32() { uapi::arch32::MINSIGSTKSZ } else { uapi::MINSIGSTKSZ };
        if ss.ss_flags & (SS_DISABLE as i32) == 0 && ss.ss_size < min_stack_size as u64 {
            return error!(ENOMEM);
        }
    }

    if !user_old_ss.is_null() {
        let mut old_ss = match state.sigaltstack() {
            Some(old_ss) => old_ss,
            None => sigaltstack { ss_flags: SS_DISABLE as i32, ..sigaltstack::default() },
        };
        if on_signal_stack {
            old_ss.ss_flags = SS_ONSTACK as i32;
        }
        current_task.write_multi_arch_object(user_old_ss, old_ss)?;
    }

    if !user_ss.is_null() {
        if ss.ss_flags & (SS_DISABLE as i32) != 0 {
            state.set_sigaltstack(None);
        } else {
            state.set_sigaltstack(Some(ss));
        }
    }

    Ok(())
}

/// The `rt_sigsuspend` syscall temporarily replaces the signal mask of the calling thread with
/// the mask given by `user_mask` and then suspends the thread until delivery of a signal whose
/// action is to invoke a signal handler or to terminate a process.
///
/// # Args
/// * `user_mask`: A pointer to a signal set that will temporarily replace the thread's signal mask.
/// * `sigset_size`: The size of the signal set, in bytes.
///
/// # Returns
/// This function never returns `Ok(())`. It always returns an `Errno`, typically `EINTR` (or a
/// restart equivalent).
pub fn sys_rt_sigsuspend(
    current_task: &mut CurrentTask,
    user_mask: UserRef<SigSet>,
    sigset_size: usize,
) -> Result<(), Errno> {
    if sigset_size != std::mem::size_of::<SigSet>() {
        return error!(EINVAL);
    }
    let mask = current_task.read_object(user_mask)?;

    let waiter = Waiter::new();
    // ERESTARTNOHAND indicates that the error should be EINTR if
    // interrupted by a signal delivered to a user handler, and the syscall
    // should be restarted otherwise.
    current_task
        .wait_with_temporary_mask(mask, |current_task| waiter.wait(current_task))
        .map_eintr(|| errno!(ERESTARTNOHAND))
}

/// The `rt_sigtimedwait` syscall waits for one of the signals in `set_addr` to become pending
/// for the calling thread. The call will block until a signal is pending or the timeout expires.
///
/// # Args
/// * `set_addr`: A pointer to a signal set specifying the signals to wait for.
/// * `siginfo_addr`: If not null, a `siginfo_t` structure for the received signal is stored here.
/// * `timeout_addr`: If not null, specifies a timeout for the wait.
/// * `sigset_size`: The size of the signal set, in bytes.
///
/// # Returns
/// On success, returns `Ok(Signal)` containing the signal that was caught. On failure, returns
/// an `Errno`.
pub fn sys_rt_sigtimedwait(
    current_task: &mut CurrentTask,
    set_addr: UserRef<SigSet>,
    siginfo_addr: MultiArchUserRef<uapi::siginfo_t, uapi::arch32::siginfo_t>,
    timeout_addr: MultiArchUserRef<uapi::timespec, uapi::arch32::timespec>,
    sigset_size: usize,
) -> Result<Signal, Errno> {
    if sigset_size != std::mem::size_of::<SigSet>() {
        return error!(EINVAL);
    }

    // Signals in `set_addr` are what we are waiting for.
    let set = current_task.read_object(set_addr)?;
    // Attempts to wait for `UNBLOCKABLE_SIGNALS` will be ignored.
    let unblock = set & !UNBLOCKABLE_SIGNALS;
    let deadline = if timeout_addr.is_null() {
        zx::MonotonicInstant::INFINITE
    } else {
        let timeout = current_task.read_multi_arch_object(timeout_addr)?;
        zx::MonotonicInstant::after(duration_from_timespec(timeout)?)
    };

    let signal_info = loop {
        let waiter;

        {
            let mut task_state = current_task.write();
            // If one of the signals in set is already pending for the calling thread,
            // sigwaitinfo() will return immediately.
            if let Some(signal) = task_state.take_signal_with_mask(!unblock) {
                break signal;
            }

            waiter = Waiter::new();
            task_state.wait_on_signal(&waiter);
        }

        // A new signal is enqueued when it's masked in the SignalState. So we need to invert
        // the SigSet to block them.
        let tmp_mask = current_task.read().signal_mask() & !unblock;

        // Wait for a timeout or a new signal.
        let waiter_result = current_task.wait_with_temporary_mask(tmp_mask, |current_task| {
            waiter.wait_until(current_task, deadline)
        });

        // Restore mask after timeout or get a new signal.
        current_task.write().restore_signal_mask();

        if let Err(e) = waiter_result {
            if e == EINTR {
                // Check if EINTR was returned for a signal we were waiting for.
                if let Some(signal) = current_task.write().take_signal_with_mask(!unblock) {
                    break signal;
                }
            } else if e == ETIMEDOUT {
                return error!(EAGAIN);
            }

            return Err(e);
        }
    };

    if !siginfo_addr.is_null() {
        signal_info.write(current_task, siginfo_addr)?;
    }

    Ok(signal_info.signal)
}

/// The `signalfd4` syscall creates a file descriptor that can be used to accept signals targeted
/// at the caller.
///
/// # Args
/// * `fd`: A file descriptor. If -1, a new file descriptor is created. Otherwise, the mask of the
///   existing signalfd is modified.
/// * `mask_addr`: A pointer to a signal set specifying the signals to handle with this signalfd.
/// * `mask_size`: The size of the signal set, in bytes.
/// * `flags`: Flags to control the behavior of the file descriptor.
///
/// # Returns
/// On success, returns `Ok(FdNumber)` containing the file descriptor number. On failure, returns
/// an `Errno`.
pub fn sys_signalfd4(
    current_task: &CurrentTask,
    fd: FdNumber,
    mask_addr: UserRef<SigSet>,
    mask_size: usize,
    flags: u32,
) -> Result<FdNumber, Errno> {
    if flags & !(SFD_CLOEXEC | SFD_NONBLOCK) != 0 {
        return error!(EINVAL);
    }
    if mask_size != std::mem::size_of::<SigSet>() {
        return error!(EINVAL);
    }
    let mask = current_task.read_object(mask_addr)?;

    if fd.raw() != -1 {
        let file = current_task.files().get(fd)?;
        let file = file.downcast_file::<SignalFd>().ok_or_else(|| errno!(EINVAL))?;
        file.set_mask(mask);
        Ok(fd)
    } else {
        let signalfd = SignalFd::new_file(current_task, mask, flags);
        let flags = if flags & SFD_CLOEXEC != 0 { FdFlags::CLOEXEC } else { FdFlags::empty() };
        let fd = current_task.add_file(signalfd, flags)?;
        Ok(fd)
    }
}

#[track_caller]
fn send_unchecked_signal(
    current_task: &CurrentTask,
    target: &Task,
    unchecked_signal: UncheckedSignal,
    si_code: i32,
) -> Result<(), Errno> {
    current_task.can_signal(&target, unchecked_signal)?;

    // 0 is a sentinel value used to do permission checks.
    if unchecked_signal.is_zero() {
        return Ok(());
    }

    let signal = Signal::try_from(unchecked_signal)?;
    security::check_signal_access(current_task, &target, signal)?;

    send_signal(
        target,
        SignalInfo::with_sender(
            signal,
            si_code,
            SignalDetail::Kill {
                pid: current_task.pid.clone(),
                uid: current_task.current_creds().uid,
            },
            Some(current_task.weak_self.clone()),
        ),
    )
}

#[track_caller]
fn send_unchecked_signal_info(
    current_task: &CurrentTask,
    target: &Task,
    unchecked_signal: UncheckedSignal,
    siginfo_ref: UserAddress,
) -> Result<(), Errno> {
    let siginfo = UncheckedSignalInfo::read_from_siginfo(current_task, siginfo_ref)?;
    if target.get_pid() != current_task.get_pid()
        && (siginfo.code() >= 0 || siginfo.code() == SI_TKILL)
    {
        return error!(EINVAL);
    }

    current_task.can_signal(&target, unchecked_signal)?;

    // 0 is a sentinel value used to do permission checks.
    if unchecked_signal.is_zero() {
        return Ok(());
    }

    let signal = Signal::try_from(unchecked_signal)?;
    security::check_signal_access(current_task, &target, signal)?;

    send_signal(&target, siginfo.into_signal_info(signal, IntoSignalInfoOptions::None)?)
}

/// The `kill` syscall can be used to send any signal to any process group or process.
///
/// # Args
/// * `pid`: Specifies the target process or process group. See `kill(2)` for details.
/// * `unchecked_signal`: The signal to send.
///
/// # Returns
/// `Ok(())` on success. Otherwise, returns an `Errno` with the error code.
pub fn sys_kill(
    current_task: &CurrentTask,
    pid: pid_t,
    unchecked_signal: UncheckedSignal,
) -> Result<(), Errno> {
    let pids = &current_task.kernel().pids;
    match pid {
        pid if pid > 0 => {
            // "If pid is positive, then signal sig is sent to the process with
            // the ID specified by pid."
            let target_thread_group = {
                let pid_entry = pids.get(pid)?;
                match pid_entry.get_process() {
                    Some(ProcessEntryRef::Process(process)) => process,

                    // Zombies cannot receive signals. Just ignore it.
                    Some(ProcessEntryRef::Zombie) => return Ok(()),

                    // If there is no process with `pid`, check if there is a task with
                    // the `pid`.
                    None => {
                        let task = pid_entry.get_task()?;
                        task.thread_group().clone()
                    }
                }
            };

            target_thread_group.send_signal_unchecked(current_task, unchecked_signal)?;
        }
        pid if pid == -1 => {
            // "If pid equals -1, then sig is sent to every process for which
            // the calling process has permission to send signals, except for
            // process 1 (init), but ... POSIX.1-2001 requires that kill(-1,sig)
            // send sig to all processes that the calling process may send
            // signals to, except possibly for some implementation-defined
            // system processes. Linux allows a process to signal itself, but on
            // Linux the call kill(-1,sig) does not signal the calling process."

            let thread_groups: Vec<_> = pids
                .get_thread_groups(&RcuReadScope::new())
                .filter(|thread_group| {
                    if *current_task.thread_group() == *thread_group {
                        return false;
                    }
                    if thread_group.leader.id == 1 {
                        return false;
                    }
                    true
                })
                .collect();
            signal_thread_groups(current_task, unchecked_signal, thread_groups)?;
        }
        _ => {
            // "If pid equals 0, then sig is sent to every process in the
            // process group of the calling process."
            //
            // "If pid is less than -1, then sig is sent to every process in the
            // process group whose ID is -pid."
            let pid = match pid {
                0 => current_task.thread_group().read().process_group.leader.clone(),
                _ => pids.get(negate_pid(pid)?)?,
            };

            let process_group = pid.get_process_group();
            let thread_groups =
                process_group.iter().flat_map(|pg| pg.read().thread_groups().collect::<Vec<_>>());
            signal_thread_groups(current_task, unchecked_signal, thread_groups)?;
        }
    };

    Ok(())
}

fn verify_tgid_for_task(task: &Task, tgid: &Pid) -> Result<(), Errno> {
    if &task.pid != tgid {
        return error!(EINVAL);
    } else {
        Ok(())
    }
}

/// The `tkill` syscall sends the signal `unchecked_signal` to the thread with the thread ID
/// `tid`.
///
/// This is an obsolete and non-standard syscall that is replaced by `tgkill`.
///
/// # Args
/// * `tid`: The thread ID of the thread to send the signal to.
/// * `unchecked_signal`: The signal to send.
///
/// # Returns
/// `Ok(())` on success. Otherwise, returns an `Errno` with the error code.
pub fn sys_tkill(
    current_task: &CurrentTask,
    tid: tid_t,
    unchecked_signal: UncheckedSignal,
) -> Result<(), Errno> {
    // Linux returns EINVAL when the tgid or tid <= 0.
    if tid <= 0 {
        return error!(EINVAL);
    }
    let thread = current_task.get_task(tid)?;
    send_unchecked_signal(current_task, &thread, unchecked_signal, SI_TKILL)
}

/// The `tgkill` syscall sends the signal `unchecked_signal` to the thread with thread ID `tid`
/// in the thread group `tgid`.
///
/// # Args
/// * `tgid`: The thread group ID of the target thread.
/// * `tid`: The thread ID of the target thread.
/// * `unchecked_signal`: The signal to send.
///
/// # Returns
/// `Ok(())` on success. Otherwise, returns an `Errno` with the error code.
pub fn sys_tgkill(
    current_task: &CurrentTask,
    tgid: pid_t,
    tid: tid_t,
    unchecked_signal: UncheckedSignal,
) -> Result<(), Errno> {
    // Linux returns EINVAL when the tgid or tid <= 0.
    if tgid <= 0 || tid <= 0 {
        return error!(EINVAL);
    }
    let pids = &current_task.kernel().pids;
    let tid = pids.get(tid)?;
    let tgid = pids.get(tgid)?;
    let thread = tid.get_task()?;
    verify_tgid_for_task(&thread, &tgid)?;
    send_unchecked_signal(current_task, &thread, unchecked_signal, SI_TKILL)
}

/// The `rt_sigreturn` syscall returns from a signal handler and restores the process's context.
///
/// This function is not intended to be called directly by user code, but is instead part of the
/// signal handling trampoline that is set up by the kernel.
///
/// # Returns
/// A `SyscallResult` with the value that should be returned to userspace. This function
/// does not return to the caller in the kernel on success.
pub fn sys_rt_sigreturn(current_task: &mut CurrentTask) -> Result<SyscallResult, Errno> {
    restore_from_signal_handler(current_task)?;
    Ok(current_task.thread_state.registers.return_register().into())
}

/// The `rt_sigqueueinfo` syscall sends a signal with a payload to a process.
///
/// # Args
/// * `tgid`: The thread group ID of the process to send the signal to.
/// * `unchecked_signal`: The signal to send.
/// * `siginfo_ref`: A pointer to a `siginfo_t` structure that contains the signal payload.
///
/// # Returns
/// `Ok(())` on success. Otherwise, returns an `Errno` with the error code.
pub fn sys_rt_sigqueueinfo(
    current_task: &CurrentTask,
    tgid: pid_t,
    unchecked_signal: UncheckedSignal,
    siginfo_ref: UserAddress,
) -> Result<(), Errno> {
    let task = current_task.get_task(tgid)?;
    task.thread_group().send_signal_unchecked_with_info(
        current_task,
        unchecked_signal,
        siginfo_ref,
        IntoSignalInfoOptions::None,
    )
}

/// The `rt_tgsigqueueinfo` syscall sends a signal with a payload to a specific thread.
///
/// # Args
/// * `tgid`: The thread group ID of the process to send the signal to.
/// * `tid`: The thread ID of the thread to send the signal to.
/// * `unchecked_signal`: The signal to send.
/// * `siginfo_ref`: A pointer to a `siginfo_t` structure that contains the signal payload.
///
/// # Returns
/// `Ok(())` on success. Otherwise, returns an `Errno` with the error code.
pub fn sys_rt_tgsigqueueinfo(
    current_task: &CurrentTask,
    tgid: pid_t,
    tid: tid_t,
    unchecked_signal: UncheckedSignal,
    siginfo_ref: UserAddress,
) -> Result<(), Errno> {
    let pids = &current_task.kernel().pids;

    let task = pids.get(tid)?.get_task()?;
    let tgid = pids.get(tgid)?;
    verify_tgid_for_task(&task, &tgid)?;
    send_unchecked_signal_info(current_task, &task, unchecked_signal, siginfo_ref)
}

/// The `pause` syscall causes the calling process sleep until it receives a signal or terminates.
///
/// # Returns
/// This function never returns `Ok(())` under normal circumstances. It always returns `Err(EINTR)`.
pub fn sys_pause(current_task: &CurrentTask) -> Result<(), Errno> {
    let event = InterruptibleEvent::new();
    let guard = event.begin_wait();
    let result = current_task.run_in_state(RunState::Event(event.clone()), || {
        match guard.block_until(None, zx::MonotonicInstant::INFINITE) {
            Err(WakeReason::Interrupted) => error!(ERESTARTNOHAND),
            Err(WakeReason::DeadlineExpired) => panic!("blocking forever cannot time out"),
            Ok(()) => Ok(()),
        }
    });
    // ERESTARTNOHAND is mapped to EINTR if interrupted by signal delivery.
    result.map_eintr(|| errno!(ERESTARTNOHAND))
}

/// The `pidfd_send_signal` syscall sends a signal to a process specified by a PID file
/// descriptor.
///
/// # Args
/// * `pidfd`: The PID file descriptor of the process to send the signal to.
/// * `unchecked_signal`: The signal to send.
/// * `siginfo_ref`: An optional pointer to a `siginfo_t` structure that contains the signal
///   payload.
/// * `flags`: Must be 0.
///
/// # Returns
/// `Ok(())` on success. Otherwise, returns an `Errno` with the error code.
pub fn sys_pidfd_send_signal(
    current_task: &CurrentTask,
    pidfd: FdNumber,
    unchecked_signal: UncheckedSignal,
    siginfo_ref: UserAddress,
    flags: u32,
) -> Result<(), Errno> {
    if flags != 0 {
        return error!(EINVAL);
    }

    let file = current_task.files().get(pidfd)?;
    let target = file.as_pid()?.get_thread_group()?;

    if siginfo_ref.is_null() {
        target.send_signal_unchecked(current_task, unchecked_signal)
    } else {
        target.send_signal_unchecked_with_info(
            current_task,
            unchecked_signal,
            siginfo_ref,
            IntoSignalInfoOptions::CheckSigno,
        )
    }
}

/// Sends a signal to all thread groups in `thread_groups`.
///
/// # Parameters
/// - `task`: The task that is sending the signal.
/// - `unchecked_signal`: The signal that is to be sent. Unchecked, since `0` is a sentinel value
/// where rights are to be checked but no signal is actually sent.
/// - `thread_groups`: The thread groups to signal.
///
/// # Returns
/// Returns Ok(()) if at least one signal was sent, otherwise the last error that was encountered.
#[track_caller]
fn signal_thread_groups<F>(
    current_task: &CurrentTask,
    unchecked_signal: UncheckedSignal,
    thread_groups: F,
) -> Result<(), Errno>
where
    F: IntoIterator<Item: AsRef<ThreadGroup>>,
{
    let mut last_error = None;
    let mut sent_signal = false;

    // This loop keeps track of whether a signal was sent, so that "on
    // success (at least one signal was sent), zero is returned."
    for thread_group in thread_groups.into_iter() {
        match thread_group.as_ref().send_signal_unchecked(current_task, unchecked_signal) {
            Ok(_) => sent_signal = true,
            Err(errno) => last_error = Some(errno),
        }
    }

    if sent_signal { Ok(()) } else { Err(last_error.unwrap_or_else(|| errno!(ESRCH))) }
}

/// The generic options for both waitid and wait4.
#[derive(Debug)]
pub struct WaitingOptions {
    /// Wait for a process that has exited.
    pub wait_for_exited: bool,
    /// Wait for a process in the stop state.
    pub wait_for_stopped: bool,
    /// Wait for a process that was continued.
    pub wait_for_continued: bool,
    /// Do not block if no child has exited.
    pub wnohang: bool,
    /// Block the wait until a process matches.
    pub block: bool,
    /// Do not clear the waitable state.
    pub keep_waitable_state: bool,
    /// Wait for all children processes.
    pub wait_for_all: bool,
    /// Wait for children who deliver no signal or a signal other than SIGCHLD, ignored if wait_for_all is true
    pub wait_for_clone: bool,
}

impl WaitingOptions {
    fn new(options: u32) -> Self {
        const_assert_eq!(WUNTRACED, WSTOPPED);
        if options & __WNOTHREAD != 0 {
            track_stub!(TODO("https://fxbug.dev/509926462"), "wait options wnothread");
        }
        Self {
            wait_for_exited: options & WEXITED > 0,
            wait_for_stopped: options & WSTOPPED > 0,
            wait_for_continued: options & WCONTINUED > 0,
            wnohang: options & WNOHANG > 0,
            block: options & WNOHANG == 0,
            keep_waitable_state: options & WNOWAIT > 0,
            wait_for_all: options & __WALL > 0,
            wait_for_clone: options & __WCLONE > 0,
        }
    }

    /// Build a `WaitingOptions` from the waiting flags of waitid.
    pub fn new_for_waitid(options: u32) -> Result<Self, Errno> {
        if options & !(__WCLONE | __WALL | WNOHANG | WNOWAIT | WSTOPPED | WEXITED | WCONTINUED) != 0
        {
            track_stub!(TODO("https://fxbug.dev/322874788"), "waitid options", options);
            return error!(EINVAL);
        }
        if options & (WEXITED | WSTOPPED | WCONTINUED) == 0 {
            return error!(EINVAL);
        }
        Ok(Self::new(options))
    }

    /// Build a `WaitingOptions` from the waiting flags of wait4.
    pub fn new_for_wait4(options: u32) -> Result<Self, Errno> {
        if options & !(__WCLONE | __WNOTHREAD | __WALL | WNOHANG | WUNTRACED | WCONTINUED) != 0 {
            track_stub!(TODO("https://fxbug.dev/322874017"), "wait4 options", options);
            return error!(EINVAL);
        }
        Ok(Self::new(options | WEXITED))
    }
}

/// Waits on the task with `pid` to exit or change state.
///
/// - `current_task`: The current task.
/// - `pid`: The id of the task to wait on.
/// - `options`: The options passed to the wait syscall.
fn wait_on_pid(
    current_task: &CurrentTask,
    selector: &ProcessSelector,
    options: &WaitingOptions,
) -> Result<Option<WaitResult>, Errno> {
    let waiter = Waiter::new();
    loop {
        {
            let mut pids = current_task.kernel().pids.lock();
            // Waits and notifies on a given task need to be done atomically
            // with respect to changes to the task's waitable state; otherwise,
            // we see missing notifications. We do that by holding the task lock.
            // This next line checks for waitable traces without holding the
            // task lock, because constructing WaitResult objects requires
            // holding all sorts of locks that are incompatible with holding the
            // task lock.  We therefore have to check to see if a tracee has
            // become waitable again, after we acquire the lock.
            if let Some(tracee) =
                current_task.thread_group().get_waitable_ptracee(selector, options, &mut pids)
            {
                return Ok(Some(tracee));
            }
            let mut has_waitable_tracee = false;
            let mut has_any_tracee = false;
            current_task.thread_group().get_ptracees_and(
                selector,
                &mut |task: &Task, task_state: &TaskMutableState| {
                    if let Some(ptrace) = &task_state.ptrace {
                        has_any_tracee = true;
                        ptrace.tracer_waiters().wait_async(&waiter);
                        if ptrace.is_waitable(task.load_stopped(), options) {
                            has_waitable_tracee = true;
                        }
                    }
                },
            );
            if has_waitable_tracee {
                continue;
            }

            {
                let mut thread_group = current_task.thread_group().write();

                if thread_group.zombie_ptracees.has_zombie_matching(&selector) {
                    continue;
                }
                match thread_group.get_waitable_child(selector, options, &mut pids) {
                    WaitableChildResult::ReadyNow(child) => {
                        return Ok(Some(*child));
                    }
                    WaitableChildResult::ShouldWait => (),
                    WaitableChildResult::NoneFound => {
                        if !has_any_tracee {
                            return error!(ECHILD);
                        }
                    }
                }
                thread_group
                    .lifecycle_waiters
                    .wait_async_value(&waiter, ThreadGroupLifecycleWaitValue::ChildStatus);
            }
        }

        if !options.block {
            return Ok(None);
        }
        waiter.wait(current_task).map_eintr(|| errno!(ERESTARTSYS))?;
    }
}

/// The `waitid` syscall waits for a child process to change state.
///
/// # Args
/// * `id_type`: The type of ID to wait for.
/// * `id`: The ID to wait for.
/// * `user_info`: A pointer to a `siginfo_t` structure that will be filled with information
///   about the state change.
/// * `options`: A bitmask of flags that control the behavior of the syscall.
/// * `user_rusage`: An optional pointer to a `rusage` structure that will be filled with
///   resource usage information.
///
/// # Returns
/// `Ok(())` on success. Otherwise, returns an `Errno` with the error code.
pub fn sys_waitid(
    current_task: &CurrentTask,
    id_type: u32,
    id: i32,
    user_info: MultiArchUserRef<uapi::siginfo_t, uapi::arch32::siginfo_t>,
    options: u32,
    user_rusage: RUsagePtr,
) -> Result<(), Errno> {
    let mut waiting_options = WaitingOptions::new_for_waitid(options)?;

    let task_selector = match id_type {
        P_PID => {
            let pid = current_task.kernel().pids.get(id).map_err(|_| errno!(ECHILD))?;
            ProcessSelector::Pid(pid)
        }
        P_ALL => ProcessSelector::Any,
        P_PGID => {
            let pid = if id == 0 {
                current_task.thread_group().read().process_group.leader.clone()
            } else {
                current_task.kernel().pids.get(id).map_err(|_| errno!(ECHILD))?
            };
            ProcessSelector::Pgid(pid)
        }
        P_PIDFD => {
            let fd = FdNumber::from_raw(id);
            let file = current_task.files().get(fd)?;
            if file.flags().contains(OpenFlags::NONBLOCK) {
                waiting_options.block = false;
            }
            ProcessSelector::Pid(file.as_pid()?)
        }
        _ => return error!(EINVAL),
    };

    // wait_on_pid returns None if no child changed state.
    if let Some(waitable_process) = wait_on_pid(current_task, &task_selector, &waiting_options)? {
        if !user_rusage.is_null() {
            let usage = rusage {
                ru_utime: timeval_from_duration(waitable_process.time_stats.user_time),
                ru_stime: timeval_from_duration(waitable_process.time_stats.system_time),
                ..Default::default()
            };

            track_stub!(TODO("https://fxbug.dev/322874712"), "real rusage from waitid");
            current_task.write_multi_arch_object(user_rusage, usage)?;
        }

        if !user_info.is_null() {
            let siginfo = waitable_process.as_signal_info();
            siginfo.write(current_task, user_info)?;
        }
    } else if id_type == P_PIDFD && !waiting_options.wnohang {
        // From <https://man7.org/linux/man-pages/man2/pidfd_open.2.html>:
        //
        //   PIDFD_NONBLOCK
        //     Return a nonblocking file descriptor.  If the process
        //     referred to by the file descriptor has not yet terminated,
        //     then an attempt to wait on the file descriptor using
        //     waitid(2) will immediately return the error EAGAIN rather
        //     than blocking.
        return error!(EAGAIN);
    } else {
        // Under Linux, when WNOHANG is specified and no child changed state,
        // waitid returns 0 and zeroes the siginfo_t structure (and rusage if provided).
        if !user_rusage.is_null() {
            current_task.write_multi_arch_object(user_rusage, rusage::default())?;
        }

        if !user_info.is_null() {
            SignalInfo::zero(current_task, user_info)?;
        }
    }

    Ok(())
}

/// The `wait4` syscall waits for a child process to change state.
///
/// # Args
/// * `raw_selector`: The PID of the process to wait for. See `wait4(2)` for more details.
/// * `user_wstatus`: A pointer to an integer that will be filled with the exit status of the
///   process.
/// * `options`: A bitmask of flags that control the behavior of the syscall.
/// * `user_rusage`: An optional pointer to a `rusage` structure that will be filled with
///   resource usage information.
///
/// # Returns
/// On success, returns the PID of the process that changed state, or 0 if `WNOHANG` was
/// specified and no child has changed state. On error, returns an `Errno`.
pub fn sys_wait4(
    current_task: &CurrentTask,
    raw_selector: pid_t,
    user_wstatus: UserRef<i32>,
    options: u32,
    user_rusage: RUsagePtr,
) -> Result<pid_t, Errno> {
    let waiting_options = WaitingOptions::new_for_wait4(options)?;

    let selector = if raw_selector == 0 {
        ProcessSelector::Pgid(current_task.thread_group().read().process_group.leader.clone())
    } else if raw_selector == -1 {
        ProcessSelector::Any
    } else if raw_selector > 0 {
        let pid = current_task.kernel().pids.get(raw_selector).map_err(|_| errno!(ECHILD))?;
        ProcessSelector::Pid(pid)
    } else if raw_selector < -1 {
        let pid = current_task
            .kernel()
            .pids
            .get(negate_pid(raw_selector)?)
            .map_err(|_| errno!(ECHILD))?;
        ProcessSelector::Pgid(pid)
    } else {
        track_stub!(
            TODO("https://fxbug.dev/322874213"),
            "wait4 with selector",
            raw_selector as u64
        );
        return error!(ENOSYS);
    };

    if let Some(waitable_process) = wait_on_pid(current_task, &selector, &waiting_options)? {
        let status = waitable_process.exit_info.status.wait_status();

        if !user_rusage.is_null() {
            track_stub!(TODO("https://fxbug.dev/322874768"), "real rusage from wait4");
            let usage = rusage {
                ru_utime: timeval_from_duration(waitable_process.time_stats.user_time),
                ru_stime: timeval_from_duration(waitable_process.time_stats.system_time),
                ..Default::default()
            };
            current_task.write_multi_arch_object(user_rusage, usage)?;
        }

        if !user_wstatus.is_null() {
            current_task.write_object(user_wstatus, &status)?;
        }

        Ok(waitable_process.pid.id)
    } else {
        Ok(0)
    }
}

// Negates the `pid` safely or fails with `ESRCH` (negation operation panics for `i32::MIN`).
fn negate_pid(pid: pid_t) -> Result<pid_t, Errno> {
    pid.checked_neg().ok_or_else(|| errno!(ESRCH))
}

// Syscalls for arch32 usage
#[cfg(target_arch = "aarch64")]
mod arch32 {
    use crate::task::CurrentTask;
    use crate::vfs::FdNumber;
    use starnix_uapi::errors::Errno;
    use starnix_uapi::signals::SigSet;
    use starnix_uapi::user_address::UserRef;

    /// The `signalfd` syscall creates a file descriptor that can be used to accept signals targeted
    /// at the caller.
    ///
    /// This is the 32-bit compatibility version of `signalfd4`.
    ///
    /// # Args
    /// * `fd`: A file descriptor. If -1, a new file descriptor is created. Otherwise, the mask of the
    ///   existing signalfd is modified.
    /// * `mask_addr`: A pointer to a signal set specifying the signals to handle with this signalfd.
    /// * `mask_size`: The size of the signal set, in bytes.
    ///
    /// # Returns
    /// On success, returns `Ok(FdNumber)` containing the file descriptor number. On failure, returns
    /// an `Errno`.
    pub fn sys_arch32_signalfd(
        current_task: &CurrentTask,
        fd: FdNumber,
        mask_addr: UserRef<SigSet>,
        mask_size: usize,
    ) -> Result<FdNumber, Errno> {
        super::sys_signalfd4(current_task, fd, mask_addr, mask_size, 0)
    }

    pub use super::{
        sys_pidfd_send_signal as sys_arch32_pidfd_send_signal,
        sys_rt_sigaction as sys_arch32_rt_sigaction,
        sys_rt_sigqueueinfo as sys_arch32_rt_sigqueueinfo,
        sys_rt_sigtimedwait as sys_arch32_rt_sigtimedwait,
        sys_rt_tgsigqueueinfo as sys_arch32_rt_tgsigqueueinfo,
        sys_sigaltstack as sys_arch32_sigaltstack, sys_signalfd4 as sys_arch32_signalfd4,
        sys_waitid as sys_arch32_waitid,
    };
}

#[cfg(target_arch = "aarch64")]
pub use arch32::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mm::{MemoryAccessor, PAGE_SIZE};
    use crate::signals::testing::dequeue_signal_for_test;
    use crate::signals::{
        SI_HEADER_SIZE, SI_MAX_SIZE_AS_USIZE, SignalInfoHeader, send_standard_signal,
    };
    use crate::task::dynamic_thread_spawner::SpawnRequestBuilder;
    use crate::task::{EventHandler, ExitStatus, ProcessExitInfo};
    use crate::testing::*;
    use starnix_sync::{EventHandlerReadyQueueLock, LockDepMutex};
    use starnix_types::math::round_up_to_system_page_size;
    use starnix_uapi::auth::Credentials;
    use starnix_uapi::errors::ERESTARTSYS;
    use starnix_uapi::signals::{
        SIGCHLD, SIGHUP, SIGINT, SIGIO, SIGKILL, SIGRTMIN, SIGSEGV, SIGSTOP, SIGTERM, SIGTRAP,
        SIGUSR1,
    };
    use starnix_uapi::vfs::FdEvents;
    use starnix_uapi::{SI_QUEUE, sigaction_t, uaddr, uid_t};
    use std::collections::VecDeque;
    use std::sync::Arc;
    use zerocopy::IntoBytes;

    #[cfg(target_arch = "x86_64")]
    #[::fuchsia::test]
    async fn test_sigaltstack() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);

            let user_ss = UserRef::<sigaltstack>::new(addr);
            let nullptr = UserRef::<sigaltstack>::default();

            // Check that the initial state is disabled.
            sys_sigaltstack(&current_task, nullptr.into(), user_ss.into())
                .expect("failed to call sigaltstack");
            let mut ss = current_task.read_object(user_ss).expect("failed to read struct");
            assert!(ss.ss_flags & (SS_DISABLE as i32) != 0);

            // Install a sigaltstack and read it back out.
            ss.ss_sp = uaddr { addr: 0x7FFFF };
            ss.ss_size = 0x1000;
            ss.ss_flags = SS_AUTODISARM as i32;
            current_task.write_object(user_ss, &ss).expect("failed to write struct");
            sys_sigaltstack(&current_task, user_ss.into(), nullptr.into())
                .expect("failed to call sigaltstack");
            current_task
                .write_memory(addr, &[0u8; std::mem::size_of::<sigaltstack>()])
                .expect("failed to clear struct");
            sys_sigaltstack(&current_task, nullptr.into(), user_ss.into())
                .expect("failed to call sigaltstack");
            let another_ss = current_task.read_object(user_ss).expect("failed to read struct");
            assert_eq!(ss.as_bytes(), another_ss.as_bytes());

            // Disable the sigaltstack and read it back out.
            let ss = sigaltstack { ss_flags: SS_DISABLE as i32, ..sigaltstack::default() };
            current_task.write_object(user_ss, &ss).expect("failed to write struct");
            sys_sigaltstack(&current_task, user_ss.into(), nullptr.into())
                .expect("failed to call sigaltstack");
            current_task
                .write_memory(addr, &[0u8; std::mem::size_of::<sigaltstack>()])
                .expect("failed to clear struct");
            sys_sigaltstack(&current_task, nullptr.into(), user_ss.into())
                .expect("failed to call sigaltstack");
            let ss = current_task.read_object(user_ss).expect("failed to read struct");
            assert!(ss.ss_flags & (SS_DISABLE as i32) != 0);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_sigaltstack_invalid_size() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);

            let user_ss = UserRef::<sigaltstack>::new(addr);
            let nullptr = UserRef::<sigaltstack>::default();

            // Check that the initial state is disabled.
            sys_sigaltstack(&current_task, nullptr.into(), user_ss.into())
                .expect("failed to call sigaltstack");
            let mut ss = current_task.read_object(user_ss).expect("failed to read struct");
            assert!(ss.ss_flags & (SS_DISABLE as i32) != 0);

            // Try to install a sigaltstack with an invalid size.
            let sigaltstack_addr_size = round_up_to_system_page_size(uapi::MINSIGSTKSZ as usize)
                .expect("failed to round up");
            let sigaltstack_addr =
                map_memory(&current_task, UserAddress::default(), sigaltstack_addr_size as u64);
            ss.ss_sp = sigaltstack_addr.into();
            ss.ss_flags = 0;
            ss.ss_size = uapi::MINSIGSTKSZ as u64 - 1;
            current_task.write_object(user_ss, &ss).expect("failed to write struct");
            assert_eq!(
                sys_sigaltstack(&current_task, user_ss.into(), nullptr.into()),
                error!(ENOMEM)
            );
        })
        .await;
    }

    #[cfg(target_arch = "x86_64")]
    #[::fuchsia::test]
    async fn test_sigaltstack_active_stack() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);

            let user_ss = UserRef::<sigaltstack>::new(addr);
            let nullptr = UserRef::<sigaltstack>::default();

            // Check that the initial state is disabled.
            sys_sigaltstack(&current_task, nullptr.into(), user_ss.into())
                .expect("failed to call sigaltstack");
            let mut ss = current_task.read_object(user_ss).expect("failed to read struct");
            assert!(ss.ss_flags & (SS_DISABLE as i32) != 0);

            // Try to install a sigaltstack.
            let sigaltstack_addr_size = round_up_to_system_page_size(uapi::MINSIGSTKSZ as usize)
                .expect("failed to round up");
            let sigaltstack_addr =
                map_memory(&current_task, UserAddress::default(), sigaltstack_addr_size as u64);
            ss.ss_sp = sigaltstack_addr.into();
            ss.ss_flags = 0;
            ss.ss_size = sigaltstack_addr_size as u64;
            current_task.write_object(user_ss, &ss).expect("failed to write struct");
            sys_sigaltstack(&current_task, user_ss.into(), nullptr.into())
                .expect("failed to call sigaltstack");

            // Changing the sigaltstack while we are there should be an error.
            let next_addr = (sigaltstack_addr + sigaltstack_addr_size).unwrap();
            current_task.thread_state.registers.rsp = next_addr.ptr() as u64;
            ss.ss_flags = SS_DISABLE as i32;
            current_task.write_object(user_ss, &ss).expect("failed to write struct");
            assert_eq!(
                sys_sigaltstack(&current_task, user_ss.into(), nullptr.into()),
                error!(EPERM)
            );

            // However, setting the rsp to a different value outside the alt stack should allow us to
            // disable it.
            let next_ss_addr = sigaltstack_addr
                .checked_add(sigaltstack_addr_size)
                .unwrap()
                .checked_add(0x1000usize)
                .unwrap();
            current_task.thread_state.registers.rsp = next_ss_addr.ptr() as u64;
            let ss = sigaltstack { ss_flags: SS_DISABLE as i32, ..sigaltstack::default() };
            current_task.write_object(user_ss, &ss).expect("failed to write struct");
            sys_sigaltstack(&current_task, user_ss.into(), nullptr.into())
                .expect("failed to call sigaltstack");
        })
        .await;
    }

    #[cfg(target_arch = "x86_64")]
    #[::fuchsia::test]
    async fn test_sigaltstack_active_stack_saturates() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);

            let user_ss = UserRef::<sigaltstack>::new(addr);
            let nullptr = UserRef::<sigaltstack>::default();

            // Check that the initial state is disabled.
            sys_sigaltstack(&current_task, nullptr.into(), user_ss.into())
                .expect("failed to call sigaltstack");
            let mut ss = current_task.read_object(user_ss).expect("failed to read struct");
            assert!(ss.ss_flags & (SS_DISABLE as i32) != 0);

            // Try to install a sigaltstack that takes the whole memory.
            let sigaltstack_addr_size = round_up_to_system_page_size(uapi::MINSIGSTKSZ as usize)
                .expect("failed to round up");
            let sigaltstack_addr =
                map_memory(&current_task, UserAddress::default(), sigaltstack_addr_size as u64);
            ss.ss_sp = sigaltstack_addr.into();
            ss.ss_flags = 0;
            ss.ss_size = u64::MAX;
            current_task.write_object(user_ss, &ss).expect("failed to write struct");
            sys_sigaltstack(&current_task, user_ss.into(), nullptr.into())
                .expect("failed to call sigaltstack");

            // Changing the sigaltstack while we are there should be an error.
            current_task.thread_state.registers.rsp =
                (sigaltstack_addr + sigaltstack_addr_size).unwrap().ptr() as u64;
            ss.ss_flags = SS_DISABLE as i32;
            current_task.write_object(user_ss, &ss).expect("failed to write struct");
            assert_eq!(
                sys_sigaltstack(&current_task, user_ss.into(), nullptr.into()),
                error!(EPERM)
            );

            // However, setting the rsp to a low value should work (it doesn't wrap-around).
            current_task.thread_state.registers.rsp = 0u64;
            let ss = sigaltstack { ss_flags: SS_DISABLE as i32, ..sigaltstack::default() };
            current_task.write_object(user_ss, &ss).expect("failed to write struct");
            sys_sigaltstack(&current_task, user_ss.into(), nullptr.into())
                .expect("failed to call sigaltstack");
        })
        .await;
    }

    /// It is invalid to call rt_sigprocmask with a sigsetsize that does not match the size of
    /// SigSet.
    #[::fuchsia::test]
    async fn test_sigprocmask_invalid_size() {
        spawn_kernel_and_run(async |current_task| {
            let set = UserRef::<SigSet>::default();
            let old_set = UserRef::<SigSet>::default();
            let how = 0;

            assert_eq!(
                sys_rt_sigprocmask(
                    &current_task,
                    how,
                    set,
                    old_set,
                    std::mem::size_of::<SigSet>() * 2
                ),
                error!(EINVAL)
            );
            assert_eq!(
                sys_rt_sigprocmask(
                    &current_task,
                    how,
                    set,
                    old_set,
                    std::mem::size_of::<SigSet>() / 2
                ),
                error!(EINVAL)
            );
        })
        .await;
    }

    /// It is invalid to call rt_sigprocmask with a bad `how`.
    #[::fuchsia::test]
    async fn test_sigprocmask_invalid_how() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);

            let set = UserRef::<SigSet>::new(addr);
            let old_set = UserRef::<SigSet>::default();
            let how = SIG_SETMASK | SIG_UNBLOCK | SIG_BLOCK;

            assert_eq!(
                sys_rt_sigprocmask(&current_task, how, set, old_set, std::mem::size_of::<SigSet>()),
                error!(EINVAL)
            );
        })
        .await;
    }

    /// It is valid to call rt_sigprocmask with a null value for set. In that case, old_set should
    /// contain the current signal mask.
    #[::fuchsia::test]
    async fn test_sigprocmask_null_set() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            let original_mask = SigSet::from(SIGTRAP);
            {
                current_task.write().set_signal_mask(original_mask);
            }

            let set = UserRef::<SigSet>::default();
            let old_set = UserRef::<SigSet>::new(addr);
            let how = SIG_SETMASK;

            current_task
                .write_memory(addr, &[0u8; std::mem::size_of::<SigSet>()])
                .expect("failed to clear struct");

            assert_eq!(
                sys_rt_sigprocmask(&current_task, how, set, old_set, std::mem::size_of::<SigSet>()),
                Ok(())
            );

            let old_mask = current_task.read_object(old_set).expect("failed to read mask");
            assert_eq!(old_mask, original_mask);
        })
        .await;
    }

    /// It is valid to call rt_sigprocmask with null values for both set and old_set.
    /// In this case, how should be ignored and the set remains the same.
    #[::fuchsia::test]
    async fn test_sigprocmask_null_set_and_old_set() {
        spawn_kernel_and_run(async |current_task| {
            let original_mask = SigSet::from(SIGTRAP);
            {
                current_task.write().set_signal_mask(original_mask);
            }

            let set = UserRef::<SigSet>::default();
            let old_set = UserRef::<SigSet>::default();
            let how = SIG_SETMASK;

            assert_eq!(
                sys_rt_sigprocmask(&current_task, how, set, old_set, std::mem::size_of::<SigSet>()),
                Ok(())
            );
            assert_eq!(current_task.read().signal_mask(), original_mask);
        })
        .await;
    }

    /// Calling rt_sigprocmask with SIG_SETMASK should set the mask to the provided set.
    #[::fuchsia::test]
    async fn test_sigprocmask_setmask() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(addr, &[0u8; std::mem::size_of::<SigSet>() * 2])
                .expect("failed to clear struct");

            let original_mask = SigSet::from(SIGTRAP);
            {
                current_task.write().set_signal_mask(original_mask);
            }

            let new_mask = SigSet::from(SIGIO);
            let set = UserRef::<SigSet>::new(addr);
            current_task.write_object(set, &new_mask).expect("failed to set mask");

            let old_addr_range = (addr + std::mem::size_of::<SigSet>()).unwrap();
            let old_set = UserRef::<SigSet>::new(old_addr_range);
            let how = SIG_SETMASK;

            assert_eq!(
                sys_rt_sigprocmask(&current_task, how, set, old_set, std::mem::size_of::<SigSet>()),
                Ok(())
            );

            let old_mask = current_task.read_object(old_set).expect("failed to read mask");
            assert_eq!(old_mask, original_mask);
            assert_eq!(current_task.read().signal_mask(), new_mask);
        })
        .await;
    }

    /// Calling st_sigprocmask with a how of SIG_BLOCK should add to the existing set.
    #[::fuchsia::test]
    async fn test_sigprocmask_block() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(addr, &[0u8; std::mem::size_of::<SigSet>() * 2])
                .expect("failed to clear struct");

            let original_mask = SigSet::from(SIGTRAP);
            {
                current_task.write().set_signal_mask(original_mask);
            }

            let new_mask = SigSet::from(SIGIO);
            let set = UserRef::<SigSet>::new(addr);
            current_task.write_object(set, &new_mask).expect("failed to set mask");

            let old_addr_range = (addr + std::mem::size_of::<SigSet>()).unwrap();
            let old_set = UserRef::<SigSet>::new(old_addr_range);
            let how = SIG_BLOCK;

            assert_eq!(
                sys_rt_sigprocmask(&current_task, how, set, old_set, std::mem::size_of::<SigSet>()),
                Ok(())
            );

            let old_mask = current_task.read_object(old_set).expect("failed to read mask");
            assert_eq!(old_mask, original_mask);
            assert_eq!(current_task.read().signal_mask(), new_mask | original_mask);
        })
        .await;
    }

    /// Calling st_sigprocmask with a how of SIG_UNBLOCK should remove from the existing set.
    #[::fuchsia::test]
    async fn test_sigprocmask_unblock() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(addr, &[0u8; std::mem::size_of::<SigSet>() * 2])
                .expect("failed to clear struct");

            let original_mask = SigSet::from(SIGTRAP) | SigSet::from(SIGIO);
            {
                current_task.write().set_signal_mask(original_mask);
            }

            let new_mask = SigSet::from(SIGTRAP);
            let set = UserRef::<SigSet>::new(addr);
            current_task.write_object(set, &new_mask).expect("failed to set mask");

            let old_addr_range = (addr + std::mem::size_of::<SigSet>()).unwrap();
            let old_set = UserRef::<SigSet>::new(old_addr_range);
            let how = SIG_UNBLOCK;

            assert_eq!(
                sys_rt_sigprocmask(&current_task, how, set, old_set, std::mem::size_of::<SigSet>()),
                Ok(())
            );

            let old_mask = current_task.read_object(old_set).expect("failed to read mask");
            assert_eq!(old_mask, original_mask);
            assert_eq!(current_task.read().signal_mask(), SIGIO.into());
        })
        .await;
    }

    /// It's ok to call sigprocmask to unblock a signal that is not set.
    #[::fuchsia::test]
    async fn test_sigprocmask_unblock_not_set() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(addr, &[0u8; std::mem::size_of::<SigSet>() * 2])
                .expect("failed to clear struct");

            let original_mask = SigSet::from(SIGIO);
            {
                current_task.write().set_signal_mask(original_mask);
            }

            let new_mask = SigSet::from(SIGTRAP);
            let set = UserRef::<SigSet>::new(addr);
            current_task.write_object(set, &new_mask).expect("failed to set mask");

            let old_addr_range = (addr + std::mem::size_of::<SigSet>()).unwrap();
            let old_set = UserRef::<SigSet>::new(old_addr_range);
            let how = SIG_UNBLOCK;

            assert_eq!(
                sys_rt_sigprocmask(&current_task, how, set, old_set, std::mem::size_of::<SigSet>()),
                Ok(())
            );

            let old_mask = current_task.read_object(old_set).expect("failed to read mask");
            assert_eq!(old_mask, original_mask);
            assert_eq!(current_task.read().signal_mask(), original_mask);
        })
        .await;
    }

    /// It's not possible to block SIGKILL or SIGSTOP.
    #[::fuchsia::test]
    async fn test_sigprocmask_kill_stop() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(addr, &[0u8; std::mem::size_of::<SigSet>() * 2])
                .expect("failed to clear struct");

            let original_mask = SigSet::from(SIGIO);
            {
                current_task.write().set_signal_mask(original_mask);
            }

            let new_mask = UNBLOCKABLE_SIGNALS;
            let set = UserRef::<SigSet>::new(addr);
            current_task.write_object(set, &new_mask).expect("failed to set mask");

            let old_addr_range = (addr + std::mem::size_of::<SigSet>()).unwrap();
            let old_set = UserRef::<SigSet>::new(old_addr_range);
            let how = SIG_BLOCK;

            assert_eq!(
                sys_rt_sigprocmask(&current_task, how, set, old_set, std::mem::size_of::<SigSet>()),
                Ok(())
            );

            let old_mask = current_task.read_object(old_set).expect("failed to read mask");
            assert_eq!(old_mask, original_mask);
            assert_eq!(current_task.read().signal_mask(), original_mask);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_sigaction_invalid_signal() {
        spawn_kernel_and_run(async |current_task| {
            assert_eq!(
                sys_rt_sigaction(
                    &current_task,
                    UncheckedSignal::from(SIGKILL),
                    // The signal is only checked when the action is set (i.e., action is non-null).
                    UserRef::<sigaction_t>::new(UserAddress::from(10)).into(),
                    UserRef::<sigaction_t>::default().into(),
                    std::mem::size_of::<SigSet>(),
                ),
                error!(EINVAL)
            );
            assert_eq!(
                sys_rt_sigaction(
                    &current_task,
                    UncheckedSignal::from(SIGSTOP),
                    // The signal is only checked when the action is set (i.e., action is non-null).
                    UserRef::<sigaction_t>::new(UserAddress::from(10)).into(),
                    UserRef::<sigaction_t>::default().into(),
                    std::mem::size_of::<SigSet>(),
                ),
                error!(EINVAL)
            );
            assert_eq!(
                sys_rt_sigaction(
                    &current_task,
                    UncheckedSignal::from(Signal::NUM_SIGNALS + 1),
                    // The signal is only checked when the action is set (i.e., action is non-null).
                    UserRef::<sigaction_t>::new(UserAddress::from(10)).into(),
                    UserRef::<sigaction_t>::default().into(),
                    std::mem::size_of::<SigSet>(),
                ),
                error!(EINVAL)
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_sigaction_old_value_set() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(addr, &[0u8; std::mem::size_of::<sigaction_t>()])
                .expect("failed to clear struct");

            let org_mask = SigSet::from(SIGHUP) | SigSet::from(SIGINT);
            let original_action =
                sigaction_t { sa_mask: org_mask.into(), ..sigaction_t::default() };

            {
                current_task.thread_group().signal_actions.set(SIGHUP, original_action);
            }

            let old_action_ref = UserRef::<sigaction_t>::new(addr);
            assert_eq!(
                sys_rt_sigaction(
                    &current_task,
                    UncheckedSignal::from(SIGHUP),
                    UserRef::<sigaction_t>::default().into(),
                    old_action_ref.into(),
                    std::mem::size_of::<SigSet>()
                ),
                Ok(())
            );

            let old_action =
                current_task.read_object(old_action_ref).expect("failed to read action");
            assert_eq!(old_action.as_bytes(), original_action.as_bytes());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_sigaction_new_value_set() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(addr, &[0u8; std::mem::size_of::<sigaction_t>()])
                .expect("failed to clear struct");

            let org_mask = SigSet::from(SIGHUP) | SigSet::from(SIGINT);
            let original_action =
                sigaction_t { sa_mask: org_mask.into(), ..sigaction_t::default() };
            let set_action_ref = UserRef::<sigaction_t>::new(addr);
            current_task
                .write_object(set_action_ref, &original_action)
                .expect("failed to set action");

            assert_eq!(
                sys_rt_sigaction(
                    &current_task,
                    UncheckedSignal::from(SIGINT),
                    set_action_ref.into(),
                    UserRef::<sigaction_t>::default().into(),
                    std::mem::size_of::<SigSet>(),
                ),
                Ok(())
            );

            assert_eq!(
                current_task.thread_group().signal_actions.get(SIGINT).as_bytes(),
                original_action.as_bytes()
            );
        })
        .await;
    }

    /// A task should be able to signal itself.
    #[::fuchsia::test]
    async fn test_kill_same_task() {
        spawn_kernel_and_run(async |current_task| {
            assert_eq!(sys_kill(&current_task, current_task.tid.id, SIGINT.into()), Ok(()));
        })
        .await;
    }

    /// A task should be able to signal its own thread group.
    #[::fuchsia::test]
    async fn test_kill_own_thread_group() {
        spawn_kernel_and_run(async |init_task| {
            let task1 = init_task.clone_task_for_test(0, Some(SIGCHLD));
            task1.thread_group().setsid().expect("setsid");
            let task2 = task1.clone_task_for_test(0, Some(SIGCHLD));

            assert_eq!(sys_kill(&task1, 0, SIGINT.into()), Ok(()));
            assert_eq!(task1.read().queued_signal_count(SIGINT), 1);
            assert_eq!(task2.read().queued_signal_count(SIGINT), 1);
            assert_eq!(init_task.read().queued_signal_count(SIGINT), 0);
        })
        .await;
    }

    /// A task should be able to signal a thread group.
    #[::fuchsia::test]
    async fn test_kill_thread_group() {
        spawn_kernel_and_run(async |init_task| {
            let task1 = init_task.clone_task_for_test(0, Some(SIGCHLD));
            task1.thread_group().setsid().expect("setsid");
            let task2 = task1.clone_task_for_test(0, Some(SIGCHLD));

            assert_eq!(sys_kill(&task1, -task1.tid.id, SIGINT.into()), Ok(()));
            assert_eq!(task1.read().queued_signal_count(SIGINT), 1);
            assert_eq!(task2.read().queued_signal_count(SIGINT), 1);
            assert_eq!(init_task.read().queued_signal_count(SIGINT), 0);
        })
        .await;
    }

    /// A task should be able to signal everything but init and itself.
    #[::fuchsia::test]
    async fn test_kill_all() {
        spawn_kernel_and_run(async |init_task| {
            let task1 = init_task.clone_task_for_test(0, Some(SIGCHLD));
            task1.thread_group().setsid().expect("setsid");
            let task2 = task1.clone_task_for_test(0, Some(SIGCHLD));

            assert_eq!(sys_kill(&task1, -1, SIGINT.into()), Ok(()));
            assert_eq!(task1.read().queued_signal_count(SIGINT), 0);
            assert_eq!(task2.read().queued_signal_count(SIGINT), 1);
            assert_eq!(init_task.read().queued_signal_count(SIGINT), 0);
        })
        .await;
    }

    /// A task should not be able to signal a nonexistent task.
    #[::fuchsia::test]
    async fn test_kill_inexistant_task() {
        spawn_kernel_and_run(async |current_task| {
            assert_eq!(sys_kill(&current_task, 9, SIGINT.into()), error!(ESRCH));
        })
        .await;
    }

    /// A task should not be able to signal a task owned by another uid.
    #[::fuchsia::test]
    async fn test_kill_invalid_task() {
        spawn_kernel_and_run(async |task1| {
            // Task must not have the kill capability.
            task1.set_creds(Credentials::with_ids(1, 1));
            let task2 = task1.clone_task_for_test(0, Some(SIGCHLD));
            task2.set_creds(Credentials::with_ids(2, 2));

            assert!(task1.can_signal(&task2, SIGINT.into()).is_err());
            assert_eq!(sys_kill(&task2, task1.tid.id, SIGINT.into()), error!(EPERM));
            assert_eq!(task1.read().queued_signal_count(SIGINT), 0);
        })
        .await;
    }

    /// A task should not be able to signal a task owned by another uid in a thead group.
    #[::fuchsia::test]
    async fn test_kill_invalid_task_in_thread_group() {
        spawn_kernel_and_run(async |init_task| {
            let task1 = init_task.clone_task_for_test(0, Some(SIGCHLD));
            task1.thread_group().setsid().expect("setsid");
            let task2 = task1.clone_task_for_test(0, Some(SIGCHLD));
            task2.thread_group().setsid().expect("setsid");
            task2.set_creds(Credentials::with_ids(2, 2));

            assert!(task2.can_signal(&task1, SIGINT.into()).is_err());
            assert_eq!(sys_kill(&task2, -task1.tid.id, SIGINT.into()), error!(EPERM));
            assert_eq!(task1.read().queued_signal_count(SIGINT), 0);
        })
        .await;
    }

    /// A task should not be able to send an invalid signal.
    #[::fuchsia::test]
    async fn test_kill_invalid_signal() {
        spawn_kernel_and_run(async |current_task| {
            assert_eq!(
                sys_kill(&current_task, current_task.tid.id, UncheckedSignal::from(75)),
                error!(EINVAL)
            );
        })
        .await;
    }

    /// Sending a blocked signal should result in a pending signal.
    #[::fuchsia::test]
    async fn test_blocked_signal_pending() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(addr, &[0u8; std::mem::size_of::<SigSet>() * 2])
                .expect("failed to clear struct");

            let new_mask = SigSet::from(SIGIO);
            let set = UserRef::<SigSet>::new(addr);
            current_task.write_object(set, &new_mask).expect("failed to set mask");

            assert_eq!(
                sys_rt_sigprocmask(
                    &current_task,
                    SIG_BLOCK,
                    set,
                    UserRef::default(),
                    std::mem::size_of::<SigSet>()
                ),
                Ok(())
            );
            assert_eq!(sys_kill(&current_task, current_task.tid.id, SIGIO.into()), Ok(()));
            assert_eq!(current_task.read().queued_signal_count(SIGIO), 1);

            // A second signal should not increment the number of pending signals.
            assert_eq!(sys_kill(&current_task, current_task.tid.id, SIGIO.into()), Ok(()));
            assert_eq!(current_task.read().queued_signal_count(SIGIO), 1);
        })
        .await;
    }

    /// More than one instance of a real-time signal can be blocked.
    #[::fuchsia::test]
    async fn test_blocked_real_time_signal_pending() {
        spawn_kernel_and_run(async |current_task| {
            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task
                .write_memory(addr, &[0u8; std::mem::size_of::<SigSet>() * 2])
                .expect("failed to clear struct");

            let new_mask = SigSet::from(starnix_uapi::signals::SIGRTMIN);
            let set = UserRef::<SigSet>::new(addr);
            current_task.write_object(set, &new_mask).expect("failed to set mask");

            assert_eq!(
                sys_rt_sigprocmask(
                    &current_task,
                    SIG_BLOCK,
                    set,
                    UserRef::default(),
                    std::mem::size_of::<SigSet>()
                ),
                Ok(())
            );
            assert_eq!(sys_kill(&current_task, current_task.tid.id, SIGRTMIN.into()), Ok(()));
            assert_eq!(current_task.read().queued_signal_count(starnix_uapi::signals::SIGRTMIN), 1);

            // A second signal should increment the number of pending signals.
            assert_eq!(sys_kill(&current_task, current_task.tid.id, SIGRTMIN.into()), Ok(()));
            assert_eq!(current_task.read().queued_signal_count(starnix_uapi::signals::SIGRTMIN), 2);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_suspend() {
        spawn_kernel_and_run(async |current_task| {
            let init_task_weak = current_task.weak_task();
            let (tx, rx) = std::sync::mpsc::sync_channel::<()>(0);

            let closure = move |current_task: &CurrentTask| {
                let init_task_temp = init_task_weak.upgrade().expect("Task must be alive");

                // Wait for the init task to be suspended.
                let mut suspended = false;
                while !suspended {
                    suspended = init_task_temp.read().is_blocked();
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }

                // Signal the suspended task with a signal that is not blocked (only SIGHUP in this test).
                let _ =
                    sys_kill(current_task, init_task_temp.tid.id, UncheckedSignal::from(SIGHUP));

                // Wait for the sigsuspend to complete.
                rx.recv().expect("receive");
                assert!(!init_task_temp.read().is_blocked());
            };
            let (thread, req) =
                SpawnRequestBuilder::new().with_sync_closure(closure).build_with_async_result();
            current_task.kernel().kthreads.spawner().spawn_from_request(req);

            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            let user_ref = UserRef::<SigSet>::new(addr);

            let sigset = !SigSet::from(SIGHUP);
            current_task.write_object(user_ref, &sigset).expect("failed to set action");

            assert_eq!(
                sys_rt_sigsuspend(current_task, user_ref, std::mem::size_of::<SigSet>()),
                error!(ERESTARTNOHAND)
            );
            tx.send(()).expect("send");
            futures::executor::block_on(thread).expect("join");
        })
        .await;
    }

    /// Waitid does not support all options.
    #[::fuchsia::test]
    async fn test_waitid_options() {
        spawn_kernel_and_run(async |current_task| {
            let id = 1;
            assert_eq!(
                sys_waitid(
                    &current_task,
                    P_PID,
                    id,
                    MultiArchUserRef::null(current_task),
                    0,
                    UserRef::default().into()
                ),
                error!(EINVAL)
            );
            assert_eq!(
                sys_waitid(
                    &current_task,
                    P_PID,
                    id,
                    MultiArchUserRef::null(current_task),
                    0xffff,
                    UserRef::default().into()
                ),
                error!(EINVAL)
            );
        })
        .await;
    }

    /// Wait4 does not support all options.
    #[::fuchsia::test]
    async fn test_wait4_options() {
        spawn_kernel_and_run(async |current_task| {
            let id = 1;
            assert_eq!(
                sys_wait4(
                    &current_task,
                    id,
                    UserRef::default(),
                    WEXITED,
                    RUsagePtr::null(current_task)
                ),
                error!(EINVAL)
            );
            assert_eq!(
                sys_wait4(
                    &current_task,
                    id,
                    UserRef::default(),
                    WNOWAIT,
                    RUsagePtr::null(current_task)
                ),
                error!(EINVAL)
            );
            assert_eq!(
                sys_wait4(
                    &current_task,
                    id,
                    UserRef::default(),
                    0xffff,
                    RUsagePtr::null(current_task)
                ),
                error!(EINVAL)
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_echild_when_no_zombie() {
        spawn_kernel_and_run(async |current_task| {
            // Send the signal to the task.
            assert!(
                sys_kill(&current_task, current_task.get_pid(), UncheckedSignal::from(SIGCHLD))
                    .is_ok()
            );
            // Verify that ECHILD is returned because there is no zombie process and no children to
            // block waiting for.
            assert_eq!(
                wait_on_pid(
                    &current_task,
                    &ProcessSelector::Any,
                    &WaitingOptions::new_for_wait4(0).expect("WaitingOptions")
                ),
                error!(ECHILD)
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_no_error_when_zombie() {
        spawn_kernel_and_run(async |current_task| {
            let child = current_task.clone_task_for_test(0, Some(SIGCHLD));
            let expected_result = WaitResult {
                pid: child.tid.clone(),
                uid: 0,
                exit_info: ProcessExitInfo {
                    status: ExitStatus::Exit(1),
                    exit_signal: Some(SIGCHLD),
                },
                time_stats: Default::default(),
            };
            child.thread_group().kill(ExitStatus::Exit(1), None);
            std::mem::drop(child);

            assert_eq!(
                wait_on_pid(
                    &current_task,
                    &ProcessSelector::Any,
                    &WaitingOptions::new_for_wait4(0).expect("WaitingOptions")
                ),
                Ok(Some(expected_result))
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_waiting_for_child() {
        spawn_kernel_and_run(async |task| {
            let child = task.clone_task_builder_for_test(0, Some(SIGCHLD));

            // No child is currently terminated.
            assert_eq!(
                wait_on_pid(
                    &task,
                    &ProcessSelector::Any,
                    &WaitingOptions::new_for_wait4(WNOHANG).expect("WaitingOptions")
                ),
                Ok(None)
            );

            let thread = std::thread::spawn({
                let task = task.weak_task();
                move || {
                    // Create child
                    let task = task.upgrade().expect("task must be alive");
                    let child: AutoReleasableTask = child.into();
                    // Wait for the main thread to be blocked on waiting for a child.
                    while !task.read().is_blocked() {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    child.thread_group().kill(ExitStatus::Exit(0), None);
                    child.tid.clone()
                }
            });

            // Block until child is terminated.
            let waited_child = wait_on_pid(
                &task,
                &ProcessSelector::Any,
                &WaitingOptions::new_for_wait4(0).expect("WaitingOptions"),
            )
            .expect("wait_on_pid")
            .unwrap();

            // Child is deleted, the thread must be able to terminate.
            let child_id = thread.join().expect("join");
            assert_eq!(waited_child.pid, child_id);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_waiting_for_child_with_signal_pending() {
        spawn_kernel_and_run(async |task| {
            // Register a signal action to ensure that the `SIGUSR1` signal interrupts the task.
            task.thread_group().signal_actions.set(
                SIGUSR1,
                sigaction_t { sa_handler: uaddr { addr: 0xDEADBEEF }, ..sigaction_t::default() },
            );

            // Start a child task. This will ensure that `wait_on_pid` tries to wait for the child.
            let _child = task.clone_task_for_test(0, Some(SIGCHLD));

            // Send a signal to the task. `wait_on_pid` should realize there is a signal pending when
            // entering a wait and return with `EINTR`.
            send_standard_signal(&task, SignalInfo::kernel(SIGUSR1));

            let errno = wait_on_pid(
                &task,
                &ProcessSelector::Any,
                &WaitingOptions::new_for_wait4(0).expect("WaitingOptions"),
            )
            .expect_err("wait_on_pid");
            assert_eq!(errno, ERESTARTSYS);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_sigkill() {
        spawn_kernel_and_run(async |current_task| {
            let mut child = current_task.clone_task_for_test(0, Some(SIGCHLD));

            // Send SIGKILL to the child. As kill is handled immediately, no need to dequeue signals.
            send_standard_signal(&child, SignalInfo::kernel(SIGKILL));
            dequeue_signal_for_test(&mut child);
            std::mem::drop(child);

            // Retrieve the exit status.
            let address = map_memory(
                &current_task,
                UserAddress::default(),
                std::mem::size_of::<i32>() as u64,
            );
            let address_ref = UserRef::<i32>::new(address);
            sys_wait4(&current_task, -1, address_ref, 0, RUsagePtr::null(current_task))
                .expect("wait4");
            let wstatus = current_task.read_object(address_ref).expect("read memory");
            assert_eq!(wstatus, SIGKILL.number() as i32);
        })
        .await;
    }

    async fn test_exit_status_for_signal(
        sig: Signal,
        wait_status: i32,
        exit_signal: Option<Signal>,
    ) {
        spawn_kernel_and_run(async move |current_task| {
            let mut child = current_task.clone_task_for_test(0, exit_signal);

            // Send the signal to the child.
            send_standard_signal(&child, SignalInfo::kernel(sig));
            dequeue_signal_for_test(&mut child);
            std::mem::drop(child);

            // Retrieve the exit status.
            let address = map_memory(
                &current_task,
                UserAddress::default(),
                std::mem::size_of::<i32>() as u64,
            );
            let address_ref = UserRef::<i32>::new(address);
            sys_wait4(&current_task, -1, address_ref, 0, RUsagePtr::null(current_task))
                .expect("wait4");
            let wstatus = current_task.read_object(address_ref).expect("read memory");
            assert_eq!(wstatus, wait_status);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_exit_status() {
        // Default action is Terminate
        test_exit_status_for_signal(SIGTERM, SIGTERM.number() as i32, Some(SIGCHLD)).await;
        // Default action is CoreDump
        test_exit_status_for_signal(SIGSEGV, (SIGSEGV.number() as i32) | 0x80, Some(SIGCHLD)).await;
    }

    #[::fuchsia::test]
    async fn test_wait4_by_pgid() {
        spawn_kernel_and_run(async |current_task| {
            let child1 = current_task.clone_task_for_test(0, Some(SIGCHLD));
            let child1_pid = child1.tid.id;
            child1.thread_group().kill(ExitStatus::Exit(42), None);
            std::mem::drop(child1);
            let child2 = current_task.clone_task_for_test(0, Some(SIGCHLD));
            child2.thread_group().setsid().expect("setsid");
            let child2_pid = child2.tid.id;
            child2.thread_group().kill(ExitStatus::Exit(42), None);
            std::mem::drop(child2);

            assert_eq!(
                sys_wait4(
                    &current_task,
                    -child2_pid,
                    UserRef::default(),
                    0,
                    RUsagePtr::null(current_task)
                ),
                Ok(child2_pid)
            );
            assert_eq!(
                sys_wait4(&current_task, 0, UserRef::default(), 0, RUsagePtr::null(current_task)),
                Ok(child1_pid)
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_waitid_by_pgid() {
        spawn_kernel_and_run(async |current_task| {
            let child1 = current_task.clone_task_for_test(0, Some(SIGCHLD));
            let child1_pid = child1.tid.id;
            child1.thread_group().kill(ExitStatus::Exit(42), None);
            std::mem::drop(child1);
            let child2 = current_task.clone_task_for_test(0, Some(SIGCHLD));
            child2.thread_group().setsid().expect("setsid");
            let child2_pid = child2.tid.id;
            child2.thread_group().kill(ExitStatus::Exit(42), None);
            std::mem::drop(child2);

            let address: UserRef<uapi::siginfo_t> =
                map_memory(&current_task, UserAddress::default(), *PAGE_SIZE).into();
            assert_eq!(
                sys_waitid(
                    &current_task,
                    P_PGID,
                    child2_pid,
                    address.into(),
                    WEXITED,
                    UserRef::default().into()
                ),
                Ok(())
            );
            // The previous wait matched child2, only child1 should be in the available zombies.
            assert_eq!(current_task.thread_group().read().zombie_children[0].pid(), child1_pid);

            assert_eq!(
                sys_waitid(
                    &current_task,
                    P_PGID,
                    0,
                    address.into(),
                    WEXITED,
                    UserRef::default().into()
                ),
                Ok(())
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_sigqueue() {
        spawn_kernel_and_run(async |current_task| {
            let current_uid = current_task.current_creds().uid;
            let current_pid = current_task.get_pid();

            const TEST_VALUE: u64 = 101;

            // Add the padding int for arch64
            const ARCH64_SI_HEADER_SIZE: usize = SI_HEADER_SIZE + 4;
            // Taken from gVisor of SignalInfo in  //pkg/abi/linux/signal.go
            const PID_DATA_OFFSET: usize = ARCH64_SI_HEADER_SIZE;
            const UID_DATA_OFFSET: usize = ARCH64_SI_HEADER_SIZE + 4;
            const VALUE_DATA_OFFSET: usize = ARCH64_SI_HEADER_SIZE + 8;

            let mut data = vec![0u8; SI_MAX_SIZE_AS_USIZE];
            let header = SignalInfoHeader {
                signo: SIGIO.number(),
                code: SI_QUEUE,
                ..SignalInfoHeader::default()
            };
            let _ = header.write_to(&mut data[..SI_HEADER_SIZE]);
            data[PID_DATA_OFFSET..PID_DATA_OFFSET + 4].copy_from_slice(&current_pid.to_ne_bytes());
            data[UID_DATA_OFFSET..UID_DATA_OFFSET + 4].copy_from_slice(&current_uid.to_ne_bytes());
            data[VALUE_DATA_OFFSET..VALUE_DATA_OFFSET + 8]
                .copy_from_slice(&TEST_VALUE.to_ne_bytes());

            let addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task.write_memory(addr, &data).unwrap();
            let second_current = create_task(current_task.kernel(), "second task");
            let second_pid = second_current.get_pid();
            let second_tid = second_current.get_tid();
            assert_eq!(second_current.read().queued_signal_count(SIGIO), 0);

            assert_eq!(
                sys_rt_tgsigqueueinfo(
                    &current_task,
                    second_pid,
                    second_tid,
                    UncheckedSignal::from(SIGIO),
                    addr
                ),
                Ok(())
            );
            assert_eq!(second_current.read().queued_signal_count(SIGIO), 1);

            let signal = SignalInfo::with_detail(
                SIGIO,
                SI_QUEUE,
                SignalDetail::Kill {
                    pid: current_task.pid.clone(),
                    uid: current_task.current_creds().uid,
                },
            );
            let queued_signal = second_current.write().take_specific_signal(signal);
            if let Some(sig) = queued_signal {
                assert_eq!(sig.signal, SIGIO);
                assert_eq!(sig.errno, 0);
                assert_eq!(sig.code, SI_QUEUE);
                if let SignalDetail::Raw { data } = sig.detail {
                    // offsets into the raw portion of the signal info
                    let offset_pid = PID_DATA_OFFSET - SI_HEADER_SIZE;
                    let offset_uid = UID_DATA_OFFSET - SI_HEADER_SIZE;
                    let offset_value = VALUE_DATA_OFFSET - SI_HEADER_SIZE;
                    let pid =
                        pid_t::from_ne_bytes(data[offset_pid..offset_pid + 4].try_into().unwrap());
                    let uid =
                        uid_t::from_ne_bytes(data[offset_uid..offset_uid + 4].try_into().unwrap());
                    let value = u64::from_ne_bytes(
                        data[offset_value..offset_value + 8].try_into().unwrap(),
                    );
                    assert_eq!(pid, current_pid);
                    assert_eq!(uid, current_uid);
                    assert_eq!(value, TEST_VALUE);
                } else {
                    panic!("incorrect signal detail");
                }
            } else {
                panic!("expected a queued signal");
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_signalfd_filters_signals() {
        spawn_kernel_and_run(async |current_task| {
            let memory_for_masks = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);

            // Create a signalfd for SIGTERM and SIGINT.
            let term_int_mask = SigSet::from(SIGTERM) | SigSet::from(SIGINT);
            let term_int_mask_addr = UserRef::<SigSet>::new(memory_for_masks);
            current_task
                .write_object(term_int_mask_addr, &term_int_mask)
                .expect("failed to write mask");
            let sfd_term_int = sys_signalfd4(
                &current_task,
                FdNumber::from_raw(-1),
                term_int_mask_addr,
                std::mem::size_of::<SigSet>(),
                0,
            )
            .expect("failed to create SIGTERM/SIGINT signalfd");

            // Create a signalfd for SIGCHLD.
            let sigchld_mask = SigSet::from(SIGCHLD);
            let sigchld_mask_addr =
                UserRef::<SigSet>::new((memory_for_masks + std::mem::size_of::<SigSet>()).unwrap());
            current_task
                .write_object(sigchld_mask_addr, &sigchld_mask)
                .expect("failed to write mask");
            let sfd_chld = sys_signalfd4(
                &current_task,
                FdNumber::from_raw(-1),
                sigchld_mask_addr,
                std::mem::size_of::<SigSet>(),
                0,
            )
            .expect("failed to create SIGCHLD signalfd");

            // Create and exit a child process, which should generate a SIGCHLD.
            let child = current_task.clone_task_for_test(0, Some(SIGCHLD));
            child.thread_group().kill(ExitStatus::Exit(1), None);
            std::mem::drop(child);

            // Check which signalfds are readable.
            let sfd_term_int_file =
                current_task.files().get(sfd_term_int).expect("failed to get sfd_term_int file");
            let sfd_chld_file =
                current_task.files().get(sfd_chld).expect("failed to get sfd_chld file");

            let term_int_events = sfd_term_int_file
                .query_events(&current_task)
                .expect("failed to query sfd_term_int events");
            let chld_events =
                sfd_chld_file.query_events(&current_task).expect("failed to query sfd_chld events");

            assert!(!term_int_events.contains(FdEvents::POLLIN));
            assert!(chld_events.contains(FdEvents::POLLIN));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_signalfd_filters_signals_async() {
        spawn_kernel_and_run(async |current_task| {
            let memory_for_masks = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);

            // Create a signalfd for SIGTERM and SIGINT.
            let term_int_mask = SigSet::from(SIGTERM) | SigSet::from(SIGINT);
            let term_int_mask_addr = UserRef::<SigSet>::new(memory_for_masks);
            current_task
                .write_object(term_int_mask_addr, &term_int_mask)
                .expect("failed to write mask");
            let sfd_term_int = sys_signalfd4(
                &current_task,
                FdNumber::from_raw(-1),
                term_int_mask_addr,
                std::mem::size_of::<SigSet>(),
                0,
            )
            .expect("failed to create SIGTERM/SIGINT signalfd");

            // Create a signalfd for SIGCHLD.
            let sigchld_mask = SigSet::from(SIGCHLD);
            let sigchld_mask_addr =
                UserRef::<SigSet>::new((memory_for_masks + std::mem::size_of::<SigSet>()).unwrap());
            current_task
                .write_object(sigchld_mask_addr, &sigchld_mask)
                .expect("failed to write mask");
            let sfd_chld = sys_signalfd4(
                &current_task,
                FdNumber::from_raw(-1),
                sigchld_mask_addr,
                std::mem::size_of::<SigSet>(),
                0,
            )
            .expect("failed to create SIGCHLD signalfd");

            // Set up the async wait.
            let waiter = Waiter::new();
            let ready_items =
                Arc::new(LockDepMutex::<_, EventHandlerReadyQueueLock>::new(VecDeque::new()));

            let sfd_term_int_file =
                current_task.files().get(sfd_term_int).expect("failed to get sfd_term_int file");
            let sfd_chld_file =
                current_task.files().get(sfd_chld).expect("failed to get sfd_chld file");

            sfd_term_int_file
                .wait_async(
                    &current_task,
                    &waiter,
                    FdEvents::POLLIN,
                    EventHandler::Enqueue {
                        key: sfd_term_int.into(),
                        queue: ready_items.clone(),
                        sought_events: FdEvents::POLLIN,
                    },
                )
                .expect("failed to wait on sfd_term_int");

            sfd_chld_file
                .wait_async(
                    &current_task,
                    &waiter,
                    FdEvents::POLLIN,
                    EventHandler::Enqueue {
                        key: sfd_chld.into(),
                        queue: ready_items.clone(),
                        sought_events: FdEvents::POLLIN,
                    },
                )
                .expect("failed to wait on sfd_chld");

            // Block SIGCHLD so it can be received by the signalfd.
            let sigchld_mask_ref = UserRef::<SigSet>::new(memory_for_masks);
            current_task
                .write_object(sigchld_mask_ref, &sigchld_mask)
                .expect("failed to write mask");
            sys_rt_sigprocmask(
                &current_task,
                SIG_BLOCK,
                sigchld_mask_ref,
                UserRef::default(),
                std::mem::size_of::<SigSet>(),
            )
            .expect("failed to block SIGCHLD");

            // Create and exit a child process, which should generate a SIGCHLD.
            let child = current_task.clone_task_for_test(0, Some(SIGCHLD));
            child.thread_group().kill(ExitStatus::Exit(1), None);
            std::mem::drop(child);

            // Wait for the signal to be processed.
            waiter.wait(&current_task).expect("failed to wait");

            // Check that only the correct signalfd was woken up.
            let ready_items = ready_items.lock();
            assert_eq!(ready_items.len(), 1);
            assert_eq!(ready_items[0].key, sfd_chld.into());
        })
        .await;
    }
}
