// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_task_command::TaskCommand;
use starnix_uapi::errors::Errno;
use starnix_uapi::{pid_t, tid_t};
use std::cell::RefCell;
use std::fmt;

// This needs to be available to the macros in this module without clients having to depend on
// log themselves.
#[doc(hidden)]
pub use log as __log;

pub use log::kv::{ToValue, Value};
pub use log::{Level, Record, logger};

/// Used to track the current thread's logical context.
enum TaskDebugInfo {
    /// The thread with this set is used for internal logic within the starnix kernel.
    Kernel,
    /// The thread with this set is used to service syscalls for a specific user thread, and this
    /// describes the user thread's identity.
    User { pid: pid_t, tid: tid_t, command: TaskCommand, leader_command: TaskCommand },
    /// Unknown info. This happens when trying to log while in the destructor of a thread local
    /// variable.
    Unknown,
}

impl TaskDebugInfo {
    pub fn leader_command(&self) -> TaskCommand {
        match self {
            Self::Kernel => TaskCommand::new(b"kthreadd"),
            Self::User { leader_command, .. } => leader_command.clone(),
            Self::Unknown => TaskCommand::new(b"<unknown>"),
        }
    }
}

thread_local! {
    /// When a thread in this kernel is started, it is a kthread by default. Once the thread
    /// becomes aware of the user-level task it is executing, this thread-local should be set to
    /// include that info.
    static CURRENT_TASK_INFO: RefCell<TaskDebugInfo> = const { RefCell::new(TaskDebugInfo::Kernel) } ;
}

impl fmt::Display for TaskDebugInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kernel => write!(f, "kthread"),
            Self::User { pid, tid, command, .. } => write!(f, "{pid}:{tid}[{command}]"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Helper type for logging macros that implements `Display` by reading the current thread's
/// `TaskDebugInfo` on demand, avoiding the need for an enclosing closure.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct CurrentTaskInfo;

impl fmt::Display for CurrentTaskInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        with_current_task_info(|task| fmt::Display::fmt(task, f))
    }
}

#[inline]
pub const fn trace_debug_logs_enabled() -> bool {
    // Allow trace and debug logs if we are in a debug (non-release) build
    // or feature `trace_and_debug_logs_in_release` is enabled.
    cfg!(debug_assertions) || cfg!(feature = "trace_and_debug_logs_in_release")
}

#[macro_export]
macro_rules! log_trace {
    ($($key:tt $(:$capture:tt)? $(= $value:expr)?),+; $($arg:tt)+) => {
        if $crate::trace_debug_logs_enabled() {
            $crate::__log::trace!(
                tag:% = $crate::CurrentTaskInfo,
                $($key $(:$capture)* $(= $value)*),+;
                $($arg)*
            );
        }
    };
    ($($arg:tt)*) => {
        if $crate::trace_debug_logs_enabled() {
            $crate::__log::trace!(tag:% = $crate::CurrentTaskInfo; $($arg)*)
        }
    };
}

#[macro_export]
macro_rules! log_syscall {
    ($current_task:expr, $($arg:tt)*) => {
        if $crate::trace_debug_logs_enabled() {
            $crate::log!(
                $current_task.task.thread_group.syscall_log_level(),
                $($arg)*
            )
        }
    };
}

#[macro_export]
macro_rules! log_debug {
    ($($key:tt $(:$capture:tt)? $(= $value:expr)?),+; $($arg:tt)+) => {
        if $crate::trace_debug_logs_enabled() {
            $crate::__log::debug!(
                tag:% = $crate::CurrentTaskInfo,
                $($key $(:$capture)* $(= $value)*),+;
                $($arg)*
            );
        }
    };
    ($($arg:tt)*) => {
        if $crate::trace_debug_logs_enabled() {
            $crate::__log::debug!(tag:% = $crate::CurrentTaskInfo; $($arg)*)
        }
    };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::log!($crate::__log::Level::Info, $($arg)*)
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::log!($crate::__log::Level::Warn, $($arg)*)
    };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::log!($crate::__log::Level::Error, $($arg)*)
    };
}

#[macro_export]
macro_rules! log {
    ($lvl:expr, $($key:tt $(:$capture:tt)? $(= $value:expr)?),+; $($arg:tt)+) => {
        $crate::__log::log!(
            $lvl,
            tag:% = $crate::CurrentTaskInfo,
            $($key $(:$capture)* $(= $value)*),+;
            $($arg)*
        )
    };
    ($lvl:expr, $($arg:tt)+) => {
        $crate::__log::log!($lvl, tag:% = $crate::CurrentTaskInfo; $($arg)*)
    };
}

// Call this when you get an error that should "never" happen, i.e. if it does that means the
// kernel was updated to produce some other error after this match was written.
#[track_caller]
pub fn impossible_error(status: zx::Status) -> Errno {
    panic!("encountered impossible error: {status}");
}

pub fn set_zx_name(obj: &impl zx::AsHandleRef, name: impl AsRef<[u8]>) {
    match obj.as_handle_ref().set_name(&zx::Name::from_bytes_lossy(name.as_ref())) {
        // ZX_ERR_BAD_STATE occurs if the target thread has exited or is in the
        // DYING/DEAD state. Allow it since the thread is in the process of tearing down.
        Ok(()) | Err(zx::Status::BAD_STATE) => {}
        Err(status) => {
            impossible_error(status);
        }
    }
}

pub fn with_zx_name<O: zx::AsHandleRef>(obj: O, name: impl AsRef<[u8]>) -> O {
    set_zx_name(&obj, name);
    obj
}

/// Set the context for log messages from this thread. Should only be called when a thread has been
/// created to execute a user-level task, and should only be called once at the start of that
/// thread's execution.
pub fn set_current_task_info(
    command: TaskCommand,
    leader_command: TaskCommand,
    pid: pid_t,
    tid: tid_t,
) {
    CURRENT_TASK_INFO.with(|task_info| {
        *task_info.borrow_mut() = TaskDebugInfo::User { pid, tid, command, leader_command };
    });
}

/// Access this thread's task info for debugging. Intended for use internally by Starnix's log
/// macros.
///
/// *Do not use this for kernel logic.* If you need access to the current pid/tid/etc for the
/// purposes of writing kernel logic beyond logging for debugging purposes, those should be accessed
/// through the `CurrentTask` type as an argument explicitly passed to your function.
#[doc(hidden)]
pub fn with_current_task_info<T>(mut f: impl FnMut(&dyn fmt::Display) -> T) -> T {
    match CURRENT_TASK_INFO.try_with(|task_info| f(&task_info.borrow())) {
        Ok(value) => value,
        Err(_) => f(&TaskDebugInfo::Unknown),
    }
}

pub(crate) fn get_current_leader_command() -> flyweights::FlyByteStr {
    match CURRENT_TASK_INFO.try_with(|task_info| task_info.borrow().leader_command()) {
        Ok(value) => value.into(),
        Err(_) => flyweights::FlyByteStr::new(b"<unknown>"),
    }
}

/// A filter for syscall logging.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SyscallLogFilter {
    match_string: String,
}

impl SyscallLogFilter {
    pub fn new(match_string: String) -> Self {
        Self { match_string }
    }

    pub fn matches(&self, command: &TaskCommand) -> bool {
        let matcher = self.match_string.as_bytes();
        command.as_bytes().windows(matcher.len()).any(|w| w == matcher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" {
        fn zx_thread_self() -> zx::sys::zx_handle_t;
    }

    #[test]
    fn test_set_zx_name_on_terminated_thread() {
        let mut terminated_thread = None;
        std::thread::scope(|s| {
            s.spawn(|| {
                terminated_thread = Some(
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    unsafe {
                        let thread = zx::Unowned::<zx::Thread>::from_raw_handle(zx_thread_self());
                        thread.duplicate_handle(zx::Rights::SAME_RIGHTS)
                    }
                    .unwrap(),
                );
            });
        });
        let terminated_thread = terminated_thread.expect("failed to obtain thread handle");
        let _ = terminated_thread
            .wait_one(zx::Signals::THREAD_TERMINATED, zx::MonotonicInstant::INFINITE);

        // The scoped thread has terminated and is in ZX_THREAD_STATE_DEAD.
        // In the Zircon microkernel, ThreadDispatcher::set_name returns ZX_ERR_BAD_STATE
        // because core_thread_ is nullptr.
        // Before fix: set_zx_name calls impossible_error(BAD_STATE) and panics.
        // After fix: set_zx_name tolerates ZX_ERR_BAD_STATE and returns Ok(()).
        set_zx_name(&terminated_thread, b"dead-thread-name");
    }

    #[test]
    #[should_panic(expected = "encountered impossible error: BAD_HANDLE")]
    #[allow(clippy::undocumented_unsafe_blocks)]
    fn test_set_zx_name_invalid_handle_panics() {
        let invalid =
            unsafe { zx::Unowned::<zx::Thread>::from_raw_handle(zx::sys::ZX_HANDLE_INVALID) };
        set_zx_name(&invalid, b"any-name");
    }
}
