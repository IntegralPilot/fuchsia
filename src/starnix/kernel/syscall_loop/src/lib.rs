// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "256"]

use anyhow::{Error, format_err};
use extended_pstate::ExtendedPstatePointer;
use starnix_core::arch::execution::new_syscall;
use starnix_core::ptrace::{StopState, ptrace_syscall_enter, ptrace_syscall_exit};
use starnix_core::signals::{
    SignalInfo, deliver_signal, dequeue_signal, prepare_to_restart_syscall,
};
use starnix_core::task::{CurrentTask, ExceptionResult, ExitStatus, SeccompStateValue, TaskFlags};
use starnix_logging::{
    CATEGORY_STARNIX, NAME_HANDLE_EXCEPTION, NAME_RESTRICTED_KICK, NAME_RUN_TASK, log_error,
    log_syscall, log_trace, log_warn, set_current_task_info,
};
use starnix_registers::RestrictedState;

use starnix_syscalls::SyscallResult;
use starnix_syscalls::decls::{Syscall, SyscallDecl};
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use starnix_uapi::signals::SIGKILL;
use zerocopy::FromZeros;

mod table;

pub fn enter(current_task: &mut CurrentTask) -> ExitStatus {
    // Zircon will populate this report on restricted exception exits. Initialize it to all zero
    // since we're just reserving storage.
    let mut exception_report = zx::sys::zx_exception_report_t::new_zeroed();
    match RestrictedState::bind_and_map(
        &mut current_task.thread_state.registers,
        &mut exception_report,
    ) {
        Ok(restricted_state) => {
            match run_task(current_task, restricted_state.bound_state.as_ptr(), &exception_report) {
                Ok(ok) => ok,
                Err(error) => {
                    log_warn!("Died unexpectedly from {error:?}! treating as SIGKILL");
                    ExitStatus::Kill(SignalInfo::kernel(SIGKILL))
                }
            }
        }
        Err(error) => {
            log_error!("failed to map mode state vmo, {error:?}! treating as SIGKILL");
            ExitStatus::Kill(SignalInfo::kernel(SIGKILL))
        }
    }
}

type RestrictedExitCallback = extern "C" fn(
    *mut RestrictedEnterContext<'_>,
    zx::sys::zx_restricted_reason_t,
    *mut ExtendedPstatePointer,
) -> bool;

unsafe extern "C" {
    // rustc doesn't like RestrictedEnterContext for FFI but we're just passing it back to
    // ourselves with extra steps.
    #[allow(improper_ctypes)]
    fn restricted_enter_loop(
        options: u32,
        restricted_exit_callback: RestrictedExitCallback,
        restricted_exit_callback_context: *mut RestrictedEnterContext<'_>,
        restricted_state: *mut zx::sys::zx_restricted_state_t,
        extended_pstate_ptr_ptr: *mut ExtendedPstatePointer,
    ) -> zx::sys::zx_status_t;
}

const RESTRICTED_ENTER_OPTIONS: u32 = 0;

struct RestrictedEnterContext<'a> {
    current_task: &'a mut CurrentTask,
    error_context: Option<ErrorContext>,
    exit_status: Result<ExitStatus, Error>,
    exception_report_raw: *const zx::sys::zx_exception_report_t,
}

/// Runs the `current_task` to completion.
///
/// The high-level flow of this function looks as follows:
///
///   1. Write the restricted state for the current thread to set it up to enter into the restricted
///      (Linux) part of the address space.
///   2. Enter restricted mode.
///   3. Return from restricted mode, reading out the new state of the restricted mode execution.
///      This state contains the thread's restricted register state, which is used to determine
///      which system call to dispatch.
///   4. Dispatch the system call.
///   5. Handle pending signals.
///   6. Goto 1.
fn run_task(
    current_task: &mut CurrentTask,
    restricted_state_ptr: *mut zx::sys::zx_restricted_state_t,
    exception_report_raw: *const zx::sys::zx_exception_report_t,
) -> Result<ExitStatus, Error> {
    set_current_task_info(
        current_task.task.command(),
        current_task.task.thread_group().read().leader_command(),
        current_task.task.pid.id,
        current_task.tid.id,
    );

    fuchsia_trace::duration!(CATEGORY_STARNIX, NAME_RUN_TASK);

    // This tracks the last failing system call for debugging purposes.
    let error_context = None;

    // We need to check for exit once, before the task starts executing, in case
    // the task has already been sent a signal that will cause it to exit.
    if let Some(exit_status) = process_completed_restricted_exit(current_task, &error_context)? {
        return Ok(exit_status);
    }

    // This extended pstate pointer points to the storage for extended processor
    // state (vector and FP registers).
    let mut extended_pstate_ptr = current_task.thread_state.extended_pstate.as_ptr();

    let mut restricted_enter_context = RestrictedEnterContext {
        current_task,
        error_context,
        exit_status: Err(errno!(ENOEXEC).into()),
        exception_report_raw,
    };

    #[allow(
        clippy::undocumented_unsafe_blocks,
        reason = "Force documented unsafe blocks in Starnix"
    )]
    let restricted_enter_status = zx::Status::ok(unsafe {
        restricted_enter_loop(
            RESTRICTED_ENTER_OPTIONS,
            restricted_exit_callback_c,
            &mut restricted_enter_context,
            restricted_state_ptr,
            &raw mut extended_pstate_ptr,
        )
    });
    if let Err(status) = restricted_enter_status {
        // If restricted_enter_loop failed, it means that we failed to satisfy
        // a prerequisite of zx_restricted_enter which should never happen.
        log_error!(
            "restricted_enter_loop failed: {}, register state: {:?}",
            status,
            restricted_enter_context.current_task.thread_state.registers
        );
    }
    restricted_enter_context.exit_status
}

extern "C" fn restricted_exit_callback_c(
    context: *mut RestrictedEnterContext<'_>,
    reason_code: zx::sys::zx_restricted_reason_t,
    extended_pstate_ptr_ptr: *mut ExtendedPstatePointer,
) -> bool {
    // SAFETY:
    // `context` is a pointer to a `RestrictedEnterContext` that was passed to
    // `restricted_enter_loop`.
    //  `extended_pstate_ptr` is a pointer to the ExtendedPstatePointer instance
    //  that was passed to `restricted_enter_loop.`
    // Our restricted return assembly and Zircon together guarantee that this
    // thread has exclusive access to these variables.
    let (restricted_context, extended_pstate_ptr) =
        unsafe { (&mut *context, extended_pstate_ptr_ptr.as_mut_unchecked()) };
    restricted_exit_callback(
        reason_code,
        restricted_context.current_task,
        &mut restricted_context.error_context,
        &mut restricted_context.exit_status,
        extended_pstate_ptr,
        restricted_context.exception_report_raw,
    )
}

fn restricted_exit_callback(
    reason_code: zx::sys::zx_restricted_reason_t,
    current_task: &mut CurrentTask,
    error_context: &mut Option<ErrorContext>,
    exit_status: &mut Result<ExitStatus, Error>,
    extended_pstate_ptr: &mut ExtendedPstatePointer,
    exception_report_raw: *const zx::sys::zx_exception_report_t,
) -> bool {
    debug_assert_eq!(
        current_task.thread_state.restart_code, None,
        "restart_code should only ever be Some() in normal mode",
    );

    let ret = match process_restricted_exit(
        reason_code,
        current_task,
        error_context,
        exception_report_raw,
    ) {
        Ok(None) => {
            // Keep going!

            *extended_pstate_ptr = current_task.thread_state.extended_pstate.as_ptr();

            true
        }
        Ok(Some(completed_exit_status)) => {
            *exit_status = Ok(completed_exit_status);
            false
        }
        Err(error) => {
            *exit_status = Err(error);
            false
        }
    };

    debug_assert_eq!(
        current_task.thread_state.restart_code, None,
        "restart_code should only ever be Some() in normal mode",
    );

    ret
}

fn process_restricted_exit(
    reason_code: zx::sys::zx_restricted_reason_t,
    current_task: &mut CurrentTask,
    error_context: &mut Option<ErrorContext>,
    exception_report_raw: *const zx::sys::zx_exception_report_t,
) -> Result<Option<ExitStatus>, Error> {
    current_task.thread_state.registers.sync_stack_ptr();

    match reason_code {
        zx::sys::ZX_RESTRICTED_REASON_SYSCALL => {
            let syscall_decl = SyscallDecl::from_number(
                current_task.thread_state.registers.syscall_register(),
                current_task.thread_state.arch_width(),
            );

            if let Some(new_error_context) = execute_syscall(current_task, syscall_decl) {
                *error_context = Some(new_error_context);
            }
        }
        zx::sys::ZX_RESTRICTED_REASON_EXCEPTION => {
            fuchsia_trace::duration!(CATEGORY_STARNIX, NAME_HANDLE_EXCEPTION);
            // SAFETY: `exception_report_raw` was written by Zircon during this restricted exit.
            let exception_report = unsafe { zx::ExceptionReport::from_raw(*exception_report_raw) };
            let exception_result = current_task.process_exception(&exception_report);
            process_completed_exception(current_task, exception_result, exception_report);
        }
        zx::sys::ZX_RESTRICTED_REASON_KICK => {
            fuchsia_trace::instant!(
                CATEGORY_STARNIX,
                NAME_RESTRICTED_KICK,
                fuchsia_trace::Scope::Thread
            );
            // Fall through to the post-syscall / post-exception handling logic. We were likely
            // kicked because a signal is pending deliver or the task has exited. Spurious kicks are
            // also possible.
        }
        _ => {
            return Err(format_err!("Received unexpected restricted reason code: {}", reason_code));
        }
    }

    if let Some(exit_status) = process_completed_restricted_exit(current_task, &error_context)? {
        return Ok(Some(exit_status));
    }

    Ok(None)
}

fn process_completed_exception(
    current_task: &mut CurrentTask,
    exception_result: ExceptionResult,
    restricted_exception: zx::ExceptionReport,
) {
    match exception_result {
        ExceptionResult::Handled => {}
        ExceptionResult::Signal(signal) => {
            let mut task_state = current_task.task.write();
            if task_state.ptrace_on_signal_consume() {
                task_state.set_stopped(
                    StopState::SignalDeliveryStopping,
                    Some(signal),
                    Some(&current_task),
                    None,
                );
                return;
            }

            if let Some(status) = deliver_signal(
                current_task.task.as_ref(),
                current_task.thread_state.arch_width(),
                task_state,
                signal.into(),
                &mut current_task.thread_state.registers,
                &current_task.thread_state.extended_pstate,
                Some(restricted_exception),
            ) {
                current_task.kill_thread_group(status);
            }
        }
    }
}

/// Contains context to track the most recently failing system call.
///
/// When a task exits with a non-zero exit code, this context is logged to help debugging which
/// system call may have triggered the failure.
#[derive(Debug)]
pub struct ErrorContext {
    /// The system call that failed.
    pub syscall: Syscall,

    /// The error that was returned for the system call.
    pub error: Errno,
}

/// Executes the provided `syscall` in `current_task`.
///
/// Returns an `ErrorContext` if the system call returned an error.
#[inline(never)] // Inlining this function breaks the CFI directives used to unwind into user code.
pub fn execute_syscall(
    current_task: &mut CurrentTask,
    syscall_decl: SyscallDecl,
) -> Option<ErrorContext> {
    fuchsia_trace::duration!(CATEGORY_STARNIX, syscall_decl.trace_name());
    let syscall = new_syscall(syscall_decl, current_task);

    current_task.thread_state.registers.save_registers_for_restart(syscall.decl.number);

    if current_task.trace_syscalls.load(std::sync::atomic::Ordering::Relaxed) {
        ptrace_syscall_enter(current_task);
    }

    log_syscall!(current_task, "{syscall:?}");

    let _lockup_detector_guard = starnix_core::task::ThreadLockupDetector::track();
    let result: Result<SyscallResult, Errno> =
        if current_task.seccomp_filter_state.get() != SeccompStateValue::None {
            // Inlined fast path for seccomp, so that we don't incur the cost
            // of a method call when running the filters.
            if let Some(res) = current_task.run_seccomp_filters(&syscall) {
                res
            } else {
                table::dispatch_syscall(current_task, &syscall)
            }
        } else {
            table::dispatch_syscall(current_task, &syscall)
        };

    current_task.trigger_delayed_releaser();

    let return_value = match result {
        Ok(return_value) => {
            log_syscall!(current_task, "-> {:#x}", return_value.value());
            current_task.thread_state.registers.set_return_register(return_value.value());
            None
        }
        Err(errno) => {
            log_syscall!(current_task, "!-> {errno}");
            if errno.is_restartable() {
                current_task.thread_state.restart_code = Some(errno.code);
            }
            current_task.thread_state.registers.set_return_register(errno.return_value());
            Some(ErrorContext { error: errno, syscall })
        }
    };

    if current_task.trace_syscalls.load(std::sync::atomic::Ordering::Relaxed) {
        ptrace_syscall_exit(current_task, return_value.is_some());
    }

    return_value
}

/// Finishes `current_task` updates after a restricted mode exit such as a syscall, exception, or kick.
///
/// Returns an `ExitStatus` if the task is meant to exit.
pub fn process_completed_restricted_exit(
    current_task: &mut CurrentTask,
    error_context: &Option<ErrorContext>,
) -> Result<Option<ExitStatus>, Errno> {
    let result;
    loop {
        // Checking for a signal might cause the task to exit, so check before processing exit
        {
            {
                if !current_task.is_exitted() {
                    dequeue_signal(current_task);
                }
                // The syscall may need to restart for a non-signal-related
                // reason. This call does nothing if we aren't restarting.
                prepare_to_restart_syscall(&mut current_task.thread_state, None);
            }
        }

        let exit_status = current_task.exit_status();
        if let Some(exit_status) = exit_status {
            log_trace!("exiting with status {:?}", exit_status);
            if let Some(error_context) = error_context {
                match exit_status {
                    ExitStatus::Exit(value) if value == 0 => {}
                    _ => {
                        log_trace!(
                            "last failing syscall before exit: {:?}, failed with {:?}",
                            error_context.syscall,
                            error_context.error
                        );
                    }
                };
            }

            result = Some(exit_status);
            break;
        } else {
            // Block a stopped process after it's had a chance to handle signals, since a signal might
            // cause it to stop.
            if current_task.block_if_stopped() {
                // If the task was stopped and has now woken up (e.g., via SIGCONT or PTRACE_CONT),
                // loop back to process any pending signals before returning to userspace.
                continue;
            }
            result = None;
            // Always restore signal mask before returning to userspace.
            current_task.write().restore_signal_mask();
            break;
        }
    }

    if let Some(ExitStatus::CoreDump(signal_info)) = &result {
        if current_task.flags().contains(TaskFlags::DUMP_ON_EXIT) {
            // Avoid taking a backtrace if the signal was sent by the same task.
            if !signal_info.is_sent_by(&current_task.weak_task()) {
                // Request a backtrace before reporting the crash to increase chance of a backtrace
                // in logs. This call is kept as far up in the call stack as possible to avoid
                // additional frames that are always the same and not relevant to users.
                // TODO(https://fxbug.dev/356732164) collect a backtrace ourselves
                debug::backtrace_request_current_thread();
            }

            if let Some(pending_report) =
                current_task.kernel().crash_reporter.begin_crash_report(&current_task)
            {
                current_task.kernel().crash_reporter.handle_core_dump(
                    &current_task,
                    signal_info,
                    pending_report,
                );
            }
        }
    }
    return Ok(result);
}
