// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon event objects.

use crate::{NullableHandle, Process, Status, Thread, ok, sys};

/// An object representing a Zircon
/// [exception object](https://fuchsia.dev/fuchsia-src/concepts/kernel/exceptions).
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Exception(NullableHandle);
impl_handle_based!(Exception);

impl Exception {
    /// Create a handle for the exception's thread
    ///
    /// Wraps the
    /// [zx_exception_get_thread](https://fuchsia.dev/fuchsia-src/reference/syscalls/exception_get_thread)
    /// syscall.
    pub fn get_thread(&self) -> Result<Thread, Status> {
        let mut handle = 0;
        let status = unsafe { sys::zx_exception_get_thread(self.raw_handle(), &mut handle) };
        ok(status)?;
        unsafe { Ok(Thread::from(NullableHandle::from_raw(handle))) }
    }

    /// Create a handle for the exception's process
    ///
    /// Wraps the
    /// [zx_exception_get_thread](https://fuchsia.dev/fuchsia-src/reference/syscalls/exception_get_thread)
    /// syscall.
    pub fn get_process(&self) -> Result<Process, Status> {
        let mut handle = 0;
        let status = unsafe { sys::zx_exception_get_process(self.raw_handle(), &mut handle) };
        ok(status)?;
        unsafe { Ok(Process::from(NullableHandle::from_raw(handle))) }
    }
}

unsafe_handle_properties!(object: Exception,
    props: [
        {query_ty: EXCEPTION_STATE, tag: ExceptionStateTag, prop_ty: sys::zx_exception_state_t, get: get_exception_state, set: set_exception_state},
    ]
);

/// Information about an exception that occurred in a process.
#[derive(Debug, Copy, Clone)]
pub struct ExceptionReport {
    /// The specific type of the exception.
    pub ty: ExceptionType,
    /// Architecture-specific information about the exception.
    pub arch: ExceptionArchData,
}

impl ExceptionReport {
    /// # Safety
    ///
    /// The provided exception report must have been written by the kernel with the `context.arch`
    /// field matching the architecture of the current device.
    pub unsafe fn from_raw(raw: sys::zx_exception_report_t) -> Self {
        debug_assert_eq!(
            raw.header.size as usize,
            std::mem::size_of::<sys::zx_exception_report_t>()
        );
        let ty = ExceptionType::from_raw(
            raw.header.type_,
            raw.context.synth_code,
            raw.context.synth_data,
        );

        // SAFETY: if the report was written by the kernel, the correct union variant will be
        // populated for each architecture.
        #[cfg(target_arch = "x86_64")]
        let arch = unsafe { ExceptionArchData::from_raw(raw.context.arch.x86_64) };
        #[cfg(target_arch = "aarch64")]
        let arch = unsafe { ExceptionArchData::from_raw(raw.context.arch.arm_64) };
        #[cfg(target_arch = "riscv64")]
        let arch = unsafe { ExceptionArchData::from_raw(raw.context.arch.riscv_64) };

        Self { ty, arch }
    }
}

/// x64-specific exception data.
#[derive(Debug, Copy, Clone)]
#[cfg(target_arch = "x86_64")]
pub struct ExceptionArchData {
    pub vector: u64,
    pub err_code: u64,
    pub cr2: u64,
}

#[cfg(target_arch = "x86_64")]
impl ExceptionArchData {
    fn from_raw(raw: sys::zx_x86_64_exc_data_t) -> Self {
        Self { vector: raw.vector, err_code: raw.err_code, cr2: raw.cr2 }
    }
}

/// arm64-specific exception data.
#[derive(Debug, Copy, Clone)]
#[cfg(target_arch = "aarch64")]
pub struct ExceptionArchData {
    pub esr: u32,
    pub far: u64,
}

#[cfg(target_arch = "aarch64")]
impl ExceptionArchData {
    fn from_raw(raw: sys::zx_arm64_exc_data_t) -> Self {
        Self { esr: raw.esr, far: raw.far }
    }
}

/// riscv64-specific exception data.
#[derive(Debug, Copy, Clone)]
#[cfg(target_arch = "riscv64")]
pub struct ExceptionArchData {
    pub cause: u64,
    pub tval: u64,
}

#[cfg(target_arch = "riscv64")]
impl ExceptionArchData {
    fn from_raw(raw: sys::zx_riscv64_exc_data_t) -> Self {
        Self { cause: raw.cause, tval: raw.tval }
    }
}

/// The type of an exception observed.
#[derive(Debug, Copy, Clone)]
pub enum ExceptionType {
    /// A general exception occurred.
    General,

    /// The process generated an unhandled page fault.
    FatalPageFault {
        /// Contains the error code returned by the page fault handler.
        status: Status,
    },

    /// The process attempted to execute an undefined CPU instruction.
    UndefinedInstruction,

    /// A software breakpoint was reached by the process.
    SoftwareBreakpoint,

    /// A hardware breakpoint was reached by the process.
    HardwareBreakpoint,

    /// The process attempted to perform an unaligned memory access.
    UnalignedAccess,

    /// A thread is starting.
    ///
    /// This exception is sent to debuggers only (ZX_EXCEPTION_CHANNEL_TYPE_DEBUGGER).
    /// The thread that generates this exception is paused until it the debugger
    /// handles the exception.
    ThreadStarting,

    /// A thread is exiting.
    ///
    /// This exception is sent to debuggers only (ZX_EXCEPTION_CHANNEL_TYPE_DEBUGGER).
    ///
    /// This exception is different from ZX_EXCP_GONE in that a debugger can
    /// still examine thread state.
    ///
    /// The thread that generates this exception is paused until it the debugger
    /// handles the exception.
    ThreadExiting,

    /// This exception is generated when a syscall fails with a job policy error (for
    /// example, an invalid handle argument is passed to the syscall when the
    /// ZX_POL_BAD_HANDLE policy is enabled) and ZX_POL_ACTION_EXCEPTION is set for
    /// the policy. The thread that generates this exception is paused until it the
    /// debugger handles the exception. Additional data about the type of policy
    /// error can be found in the |synth_code| field of the report and will be a
    /// ZX_EXCP_POLICY_CODE_* value.
    PolicyError(PolicyCode),

    /// A process is starting.
    ///
    /// This exception is sent to job debuggers only (ZX_EXCEPTION_CHANNEL_TYPE_JOB_DEBUGGER).
    ///
    /// The thread that generates this exception is paused until it the debugger
    /// handles the exception.
    ProcessStarting,

    /// A user-generated exception indicating to a debugger that the process' name has changed.
    ProcessNameChanged,

    /// The exception was user-generated but of an unknown type.
    UnknownUserGenerated { code: u32, data: u32 },

    /// An unknown exception type.
    Unknown { ty: u32, code: u32, data: u32 },
}

impl ExceptionType {
    fn from_raw(raw: sys::zx_excp_type_t, code: u32, data: u32) -> Self {
        match raw {
            sys::ZX_EXCP_GENERAL => Self::General,
            sys::ZX_EXCP_FATAL_PAGE_FAULT => {
                Self::FatalPageFault { status: Status::err_from_raw(code as i32) }
            }
            sys::ZX_EXCP_UNDEFINED_INSTRUCTION => Self::UndefinedInstruction,
            sys::ZX_EXCP_SW_BREAKPOINT => Self::SoftwareBreakpoint,
            sys::ZX_EXCP_HW_BREAKPOINT => Self::HardwareBreakpoint,
            sys::ZX_EXCP_UNALIGNED_ACCESS => Self::UnalignedAccess,
            sys::ZX_EXCP_THREAD_STARTING => Self::ThreadStarting,
            sys::ZX_EXCP_THREAD_EXITING => Self::ThreadExiting,
            sys::ZX_EXCP_POLICY_ERROR => Self::PolicyError(PolicyCode::from_raw(code, data)),
            sys::ZX_EXCP_PROCESS_STARTING => Self::ProcessStarting,
            sys::ZX_EXCP_USER => match code {
                sys::ZX_EXCP_USER_CODE_PROCESS_NAME_CHANGED => Self::ProcessNameChanged,
                _ => Self::UnknownUserGenerated { code, data },
            },
            other => Self::Unknown { ty: other, code, data },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum PolicyCode {
    BadHandle,
    WrongObject,
    VmarWriteExecutable,
    NewAny,
    NewVmo,
    NewChannel,
    NewEvent,
    NewEventPair,
    NewPort,
    NewSocket,
    NewFifo,
    NewTimer,
    NewProcess,
    NewProfile,
    NewPager,
    AmbientMarkVmoExecutable,
    ChannelFullWrite,
    PortTooManyPackets,
    BadSyscall {
        number: u32,
    },
    PortTooManyObservers,

    /// An invalid zx_handle_t* was passed to a syscall, resulting in a handle leak.
    /// This exception is generated when a thread fails to provide a valid target
    /// handle buffer for syscalls that return handles. This is temporary until we
    /// have fully migrated to HandleTableV3.
    HandleLeak,
    NewIob,

    /// An unknown policy error.
    Unknown {
        code: u32,
        data: u32,
    },
}

impl PolicyCode {
    fn from_raw(code: u32, data: u32) -> Self {
        match code {
            sys::ZX_EXCP_POLICY_CODE_BAD_HANDLE => Self::BadHandle,
            sys::ZX_EXCP_POLICY_CODE_WRONG_OBJECT => Self::WrongObject,
            sys::ZX_EXCP_POLICY_CODE_VMAR_WX => Self::VmarWriteExecutable,
            sys::ZX_EXCP_POLICY_CODE_NEW_ANY => Self::NewAny,
            sys::ZX_EXCP_POLICY_CODE_NEW_VMO => Self::NewVmo,
            sys::ZX_EXCP_POLICY_CODE_NEW_CHANNEL => Self::NewChannel,
            sys::ZX_EXCP_POLICY_CODE_NEW_EVENT => Self::NewEvent,
            sys::ZX_EXCP_POLICY_CODE_NEW_EVENTPAIR => Self::NewEventPair,
            sys::ZX_EXCP_POLICY_CODE_NEW_PORT => Self::NewPort,
            sys::ZX_EXCP_POLICY_CODE_NEW_SOCKET => Self::NewSocket,
            sys::ZX_EXCP_POLICY_CODE_NEW_FIFO => Self::NewFifo,
            sys::ZX_EXCP_POLICY_CODE_NEW_TIMER => Self::NewTimer,
            sys::ZX_EXCP_POLICY_CODE_NEW_PROCESS => Self::NewProcess,
            sys::ZX_EXCP_POLICY_CODE_NEW_PROFILE => Self::NewProfile,
            sys::ZX_EXCP_POLICY_CODE_NEW_PAGER => Self::NewPager,
            sys::ZX_EXCP_POLICY_CODE_AMBIENT_MARK_VMO_EXEC => Self::AmbientMarkVmoExecutable,
            sys::ZX_EXCP_POLICY_CODE_CHANNEL_FULL_WRITE => Self::ChannelFullWrite,
            sys::ZX_EXCP_POLICY_CODE_PORT_TOO_MANY_PACKETS => Self::PortTooManyPackets,
            sys::ZX_EXCP_POLICY_CODE_BAD_SYSCALL => Self::BadSyscall { number: data },
            sys::ZX_EXCP_POLICY_CODE_PORT_TOO_MANY_OBSERVERS => Self::PortTooManyObservers,
            sys::ZX_EXCP_POLICY_CODE_HANDLE_LEAK => Self::HandleLeak,
            sys::ZX_EXCP_POLICY_CODE_NEW_IOB => Self::NewIob,
            _ => Self::Unknown { code, data },
        }
    }
}
