// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon threads.

use crate::{
    ExceptionReport, MonotonicDuration, NullableHandle, ObjectQuery, Profile, Status, Task, Topic,
    ok, sys,
};
use bitflags::bitflags;

bitflags! {
    /// Options that may be used with `Thread::raise_exception`
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct RaiseExceptionOptions: u32 {
        const TARGET_JOB_DEBUGGER = sys::ZX_EXCEPTION_TARGET_JOB_DEBUGGER;
    }
}

/// An object representing a Zircon thread.
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Thread(NullableHandle);
impl_handle_based!(Thread);

struct ThreadExceptionReport;
unsafe impl ObjectQuery for ThreadExceptionReport {
    const TOPIC: Topic = Topic::THREAD_EXCEPTION_REPORT;
    type InfoTy = sys::zx_exception_report_t;
}

impl Thread {
    /// Cause the thread to begin execution.
    ///
    /// Wraps the
    /// [zx_thread_start](https://fuchsia.dev/fuchsia-src/reference/syscalls/thread_start.md)
    /// syscall.
    pub fn start(
        &self,
        thread_entry: usize,
        stack: usize,
        arg1: usize,
        arg2: usize,
    ) -> Result<(), Status> {
        let thread_raw = self.raw_handle();
        let status = unsafe { sys::zx_thread_start(thread_raw, thread_entry, stack, arg1, arg2) };
        ok(status)
    }

    /// Apply a scheduling profile to a thread.
    ///
    /// Wraps the
    /// [zx_object_set_profile](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_set_profile) syscall.
    pub fn set_profile(&self, profile: &Profile, options: u32) -> Result<(), Status> {
        let thread_raw = self.raw_handle();
        let profile_raw = profile.raw_handle();
        let status = unsafe { sys::zx_object_set_profile(thread_raw, profile_raw, options) };
        ok(status)
    }

    /// Terminate the current running thread.
    ///
    /// # Safety
    ///
    /// Extreme caution should be used-- this is basically always UB in Rust.
    /// There's almost no "normal" program code where this is okay to call.
    /// Users should take care that no references could possibly exist to
    /// stack variables on this thread, and that any destructors, closure
    /// suffixes, or other "after this thing runs" code is waiting to run
    /// in order for safety.
    pub unsafe fn exit() {
        unsafe { sys::zx_thread_exit() }
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_THREAD_EXCEPTION_REPORT topic.
    pub fn exception_report(&self) -> Result<ExceptionReport, Status> {
        let raw = self.0.get_info_single::<ThreadExceptionReport>()?;

        // SAFETY: this value was provided by the kernel, the union is valid.
        Ok(unsafe { ExceptionReport::from_raw(raw) })
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_THREAD topic.
    pub fn info(&self) -> Result<ThreadInfo, Status> {
        Ok(ThreadInfo::from_raw(self.0.get_info_single::<ThreadInfoQuery>()?))
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_THREAD_STATS topic.
    pub fn stats(&self) -> Result<ThreadStats, Status> {
        Ok(ThreadStats::from_raw(self.0.get_info_single::<ThreadStatsQuery>()?))
    }

    pub fn read_state_general_regs(&self) -> Result<sys::zx_thread_state_general_regs_t, Status> {
        let mut state = sys::zx_thread_state_general_regs_t::default();
        let thread_raw = self.raw_handle();
        let status = unsafe {
            sys::zx_thread_read_state(
                thread_raw,
                sys::ZX_THREAD_STATE_GENERAL_REGS,
                std::ptr::from_mut(&mut state).cast::<u8>(),
                std::mem::size_of_val(&state),
            )
        };
        ok(status).map(|_| state)
    }

    pub fn write_state_general_regs(
        &self,
        state: sys::zx_thread_state_general_regs_t,
    ) -> Result<(), Status> {
        let thread_raw = self.raw_handle();
        let status = unsafe {
            sys::zx_thread_write_state(
                thread_raw,
                sys::ZX_THREAD_STATE_GENERAL_REGS,
                std::ptr::from_ref(&state).cast::<u8>(),
                std::mem::size_of_val(&state),
            )
        };
        ok(status)
    }

    /// Wraps the `zx_thread_raise_exception` syscall.
    ///
    /// See https://fuchsia.dev/reference/syscalls/thread_raise_exception?hl=en for details.
    pub fn raise_user_exception(
        options: RaiseExceptionOptions,
        synth_code: u32,
        synth_data: u32,
    ) -> Result<(), Status> {
        let arch = unsafe { std::mem::zeroed::<sys::zx_exception_header_arch_t>() };
        let context = sys::zx_exception_context_t { arch, synth_code, synth_data };

        // SAFETY: basic FFI call. `&context` is a valid pointer.
        ok(unsafe { sys::zx_thread_raise_exception(options.bits(), sys::ZX_EXCP_USER, &context) })
    }
}

impl Task for Thread {}

struct ThreadStatsQuery;
unsafe impl ObjectQuery for ThreadStatsQuery {
    const TOPIC: Topic = Topic::THREAD_STATS;
    type InfoTy = sys::zx_info_thread_stats_t;
}

#[cfg(target_arch = "x86_64")]
unsafe_handle_properties!(object: Thread,
    props: [
        {query_ty: REGISTER_GS, tag: RegisterGsTag, prop_ty: usize, set: set_register_gs},
        {query_ty: REGISTER_FS, tag: RegisterFsTag, prop_ty: usize, set: set_register_fs},
    ]
);

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub struct ThreadStats {
    pub total_runtime: MonotonicDuration,
    pub last_scheduled_cpu: u32,
}

impl ThreadStats {
    fn from_raw(
        sys::zx_info_thread_stats_t { total_runtime, last_scheduled_cpu, .. }: sys::zx_info_thread_stats_t,
    ) -> Self {
        Self { total_runtime: MonotonicDuration::from_nanos(total_runtime), last_scheduled_cpu }
    }
}

struct ThreadInfoQuery;
unsafe impl ObjectQuery for ThreadInfoQuery {
    const TOPIC: Topic = Topic::THREAD;
    type InfoTy = sys::zx_info_thread_t;
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct ThreadInfo {
    pub state: ThreadState,
    pub cpu_affinity_mask: sys::zx_cpu_set_t,
}

impl ThreadInfo {
    fn from_raw(raw: sys::zx_info_thread_t) -> Self {
        Self {
            state: ThreadState::from_raw(raw.state, raw.wait_exception_channel_type),
            cpu_affinity_mask: raw.cpu_affinity_mask,
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ThreadState {
    New,
    Running,
    Suspended,
    Blocked(ThreadBlockType),
    Dying,
    Dead,
    Unknown(u32),
}

impl ThreadState {
    fn from_raw(raw_state: u32, raw_wait_exception_channel_type: u32) -> Self {
        match raw_state {
            sys::ZX_THREAD_STATE_NEW => Self::New,
            sys::ZX_THREAD_STATE_RUNNING => Self::Running,
            sys::ZX_THREAD_STATE_SUSPENDED => Self::Suspended,
            sys::ZX_THREAD_STATE_BLOCKED => Self::Blocked(ThreadBlockType::Unknown),
            sys::ZX_THREAD_STATE_BLOCKED_EXCEPTION => Self::Blocked(ThreadBlockType::Exception(
                ExceptionChannelType::from_raw(raw_wait_exception_channel_type),
            )),
            sys::ZX_THREAD_STATE_BLOCKED_SLEEPING => Self::Blocked(ThreadBlockType::Sleeping),
            sys::ZX_THREAD_STATE_BLOCKED_FUTEX => Self::Blocked(ThreadBlockType::Futex),
            sys::ZX_THREAD_STATE_BLOCKED_PORT => Self::Blocked(ThreadBlockType::Port),
            sys::ZX_THREAD_STATE_BLOCKED_CHANNEL => Self::Blocked(ThreadBlockType::Channel),
            sys::ZX_THREAD_STATE_BLOCKED_WAIT_ONE => Self::Blocked(ThreadBlockType::WaitOne),
            sys::ZX_THREAD_STATE_BLOCKED_WAIT_MANY => Self::Blocked(ThreadBlockType::WaitMany),
            sys::ZX_THREAD_STATE_BLOCKED_INTERRUPT => Self::Blocked(ThreadBlockType::Interrupt),
            sys::ZX_THREAD_STATE_BLOCKED_PAGER => Self::Blocked(ThreadBlockType::Pager),
            sys::ZX_THREAD_STATE_DYING => Self::Dying,
            sys::ZX_THREAD_STATE_DEAD => Self::Dead,
            _ => Self::Unknown(raw_state),
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ThreadBlockType {
    Exception(ExceptionChannelType),
    Sleeping,
    Futex,
    Port,
    Channel,
    WaitOne,
    WaitMany,
    Interrupt,
    Pager,
    Unknown,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ExceptionChannelType {
    None,
    Debugger,
    Thread,
    Process,
    Job,
    JobDebugger,
    Unknown(u32),
}

impl ExceptionChannelType {
    fn from_raw(raw_wait_exception_channel_type: u32) -> Self {
        match raw_wait_exception_channel_type {
            sys::ZX_EXCEPTION_CHANNEL_TYPE_NONE => Self::None,
            sys::ZX_EXCEPTION_CHANNEL_TYPE_DEBUGGER => Self::Debugger,
            sys::ZX_EXCEPTION_CHANNEL_TYPE_THREAD => Self::Thread,
            sys::ZX_EXCEPTION_CHANNEL_TYPE_PROCESS => Self::Process,
            sys::ZX_EXCEPTION_CHANNEL_TYPE_JOB => Self::Job,
            sys::ZX_EXCEPTION_CHANNEL_TYPE_JOB_DEBUGGER => Self::JobDebugger,
            _ => Self::Unknown(raw_wait_exception_channel_type),
        }
    }
}
