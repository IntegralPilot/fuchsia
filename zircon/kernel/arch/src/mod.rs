// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#[cfg(target_arch = "aarch64")]
#[path = "../arm64/src/mod.rs"]
pub mod arch_arm64;

#[cfg(target_arch = "aarch64")]
#[allow(unused_imports)]
pub use arch_arm64::{self as arm64, *};

#[cfg(target_arch = "riscv64")]
#[path = "../riscv64/src/mod.rs"]
pub mod arch_riscv64;

#[cfg(target_arch = "riscv64")]
#[allow(unused_imports)]
pub use arch_riscv64::{self as riscv64, *};

#[cfg(target_arch = "x86_64")]
#[path = "../x86/src/mod.rs"]
pub mod arch_x86;

#[cfg(target_arch = "x86_64")]
#[allow(unused_imports)]
pub use arch_x86::{self as x86, *};

pub use arch_types_bindings::{GeneralRegsSource, UserEntryState};

#[macro_use]
pub mod api;

pub mod ops;

#[cfg(target_arch = "riscv64")]
assert_arch_signatures!(riscv64);

use crate::kernel::percpu::PerCpu;
use zx_status::Status;

unsafe extern "C" {
    fn cpp_arch_blocking_disallowed() -> bool;
    fn cpp_arch_ints_disabled() -> bool;
    fn cpp_arch_disable_ints();
    fn cpp_arch_enable_ints();
    fn cpp_arch_interrupt_save() -> InterruptSavedState;
    fn cpp_arch_interrupt_restore(state: InterruptSavedState);
    fn cpp_arch_curr_cpu_num() -> u32;
    fn cpp_arch_max_num_cpus() -> u32;
    fn cpp_arch_set_blocking_disallowed(value: bool);
    fn cpp_arch_set_restricted_flag(restricted: bool);
    fn cpp_arch_get_curr_percpu() -> *const core::ffi::c_void;
}

#[cfg(not(target_arch = "riscv64"))]
unsafe extern "C" {
    fn cpp_arch_copy_from_user(
        dst: *mut core::ffi::c_void,
        src: *const core::ffi::c_void,
        len: usize,
    ) -> i32;
    fn cpp_arch_copy_to_user(
        dst: *mut core::ffi::c_void,
        src: *const core::ffi::c_void,
        len: usize,
    ) -> i32;
    fn cpp_arch_copy_from_user_capture_faults(
        dst: *mut core::ffi::c_void,
        src: *const core::ffi::c_void,
        len: usize,
        fault_va: *mut usize,
        fault_flags: *mut u32,
    ) -> i32;
    fn cpp_arch_copy_to_user_capture_faults(
        dst: *mut core::ffi::c_void,
        src: *const core::ffi::c_void,
        len: usize,
        fault_va: *mut usize,
        fault_flags: *mut u32,
    ) -> i32;
}

/// Sets the architecture-specific restricted mode flag on the current CPU.
pub fn set_restricted_flag(restricted: bool) {
    // SAFETY: Foreign function call into architecture-specific restricted mode flag setting.
    unsafe { cpp_arch_set_restricted_flag(restricted) }
}

/// The arch_blocking_disallowed() flag is used to check that in-kernel interrupt
/// handlers do not do any blocking operations.  This is a per-CPU flag.
/// Various blocking operations, such as mutex.Acquire(), contain assertions
/// that arch_blocking_disallowed() is false.
///
/// arch_blocking_disallowed() should only be true when interrupts are
/// disabled.
#[inline(always)]
pub fn blocking_disallowed() -> bool {
    unsafe { cpp_arch_blocking_disallowed() }
}

/// Returns true if interrupts are disabled on the current CPU.
#[inline(always)]
pub fn ints_disabled() -> bool {
    unsafe { cpp_arch_ints_disabled() }
}

/// Disable interrupts on the current CPU.
#[inline(always)]
pub fn disable_ints() {
    unsafe { cpp_arch_disable_ints() }
}

/// Enable interrupts on the current CPU.
#[inline(always)]
pub fn enable_ints() {
    unsafe { cpp_arch_enable_ints() }
}

/// The saved interrupt state, representing architecture-specific interrupt flags.
#[cfg(target_arch = "x86_64")]
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterruptSavedState(usize);

/// The saved interrupt state, representing architecture-specific interrupt flags.
#[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterruptSavedState(bool);

/// Save the current interrupt state (specifically, whether interrupts are enabled)
/// and disable interrupts on the current CPU.
#[inline(always)]
pub fn arch_interrupt_save() -> InterruptSavedState {
    unsafe { cpp_arch_interrupt_save() }
}

/// Restore the interrupt state on the current CPU to a previously saved state.
#[inline(always)]
pub fn arch_interrupt_restore(state: InterruptSavedState) {
    unsafe { cpp_arch_interrupt_restore(state) }
}

/// A guard that disables interrupts on the current CPU when created,
/// and restores the previous interrupt state when dropped.
pub struct InterruptDisableGuard {
    state: InterruptSavedState,
}

impl InterruptDisableGuard {
    #[inline(always)]
    pub fn new() -> Self {
        Self { state: arch_interrupt_save() }
    }
}

impl Drop for InterruptDisableGuard {
    #[inline(always)]
    fn drop(&mut self) {
        arch_interrupt_restore(self.state);
    }
}

/// Returns the CPU number of the calling CPU.
#[inline(always)]
pub fn curr_cpu_num() -> u32 {
    unsafe { cpp_arch_curr_cpu_num() }
}

/// Returns the maximum number of CPUs in the system.
#[inline(always)]
pub fn max_num_cpus() -> u32 {
    unsafe { cpp_arch_max_num_cpus() }
}

/// Return a pointer to the high-level percpu struct for the calling CPU.
#[inline(always)]
pub fn get_curr_percpu() -> *const PerCpu {
    unsafe { cpp_arch_get_curr_percpu().cast() }
}

/// Copies `len` bytes from user memory at `src` into kernel memory at `dst`.
///
/// # Safety
/// Caller must ensure `dst` points to at least `len` bytes of valid memory
/// and `src` is a user pointer.
#[inline(always)]
pub unsafe fn arch_copy_from_user(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    len: usize,
) -> Result<(), Status> {
    #[cfg(target_arch = "riscv64")]
    {
        // SAFETY: Caller guarantees valid pointers and safety invariants.
        unsafe { riscv64::arch_copy_from_user(dst, src, len) }
    }
    #[cfg(not(target_arch = "riscv64"))]
    {
        // SAFETY: Caller guarantees valid pointers and safety invariants.
        Status::ok(unsafe { cpp_arch_copy_from_user(dst, src, len) })
    }
}

/// Copies `len` bytes from kernel memory at `src` into user memory at `dst`.
///
/// # Safety
/// Caller must ensure `src` points to at least `len` bytes of valid memory
/// and `dst` is a user pointer.
#[inline(always)]
pub unsafe fn arch_copy_to_user(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    len: usize,
) -> Result<(), Status> {
    #[cfg(target_arch = "riscv64")]
    {
        // SAFETY: Caller guarantees valid pointers and safety invariants.
        unsafe { riscv64::arch_copy_to_user(dst, src, len) }
    }
    #[cfg(not(target_arch = "riscv64"))]
    {
        // SAFETY: Caller guarantees valid pointers and safety invariants.
        Status::ok(unsafe { cpp_arch_copy_to_user(dst, src, len) })
    }
}

/// Page fault information captured during a user copy operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultInfo {
    pub pf_va: usize,
    pub pf_flags: u32,
}

/// Error type returned by user copy routines that capture page faults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserCopyCaptureFaultsError {
    pub status: Status,
    pub fault_info: Option<FaultInfo>,
}

impl From<Status> for UserCopyCaptureFaultsError {
    fn from(status: Status) -> Self {
        Self { status, fault_info: None }
    }
}

#[cfg(not(target_arch = "riscv64"))]
#[inline(always)]
fn capture_faults_result(
    status: i32,
    fault_va: usize,
    fault_flags: u32,
) -> Result<(), UserCopyCaptureFaultsError> {
    if let Err(status) = Status::ok(status) {
        let fault_info = if fault_va != 0 || fault_flags != 0 {
            Some(FaultInfo { pf_va: fault_va, pf_flags: fault_flags })
        } else {
            None
        };
        Err(UserCopyCaptureFaultsError { status, fault_info })
    } else {
        Ok(())
    }
}

/// Copies `len` bytes from user memory at `src` into kernel memory at `dst`, capturing any page
/// faults.
///
/// # Safety
/// Caller must ensure `dst` points to at least `len` bytes of valid memory
/// and `src` is a user pointer.
#[inline(always)]
pub unsafe fn arch_copy_from_user_capture_faults(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    len: usize,
) -> Result<(), UserCopyCaptureFaultsError> {
    #[cfg(target_arch = "riscv64")]
    {
        // SAFETY: Caller guarantees valid pointers and safety invariants.
        unsafe { riscv64::arch_copy_from_user_capture_faults(dst, src, len) }
    }
    #[cfg(not(target_arch = "riscv64"))]
    {
        let mut fault_va = 0usize;
        let mut fault_flags = 0u32;
        // SAFETY: Foreign function call with valid pointer arguments.
        let status = unsafe {
            cpp_arch_copy_from_user_capture_faults(
                dst,
                src,
                len,
                &raw mut fault_va,
                &raw mut fault_flags,
            )
        };
        capture_faults_result(status, fault_va, fault_flags)
    }
}

/// Copies `len` bytes from kernel memory at `src` into user memory at `dst`, capturing any page
/// faults.
///
/// # Safety
/// Caller must ensure `src` points to at least `len` bytes of valid memory
/// and `dst` is a user pointer.
#[inline(always)]
pub unsafe fn arch_copy_to_user_capture_faults(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    len: usize,
) -> Result<(), UserCopyCaptureFaultsError> {
    #[cfg(target_arch = "riscv64")]
    {
        // SAFETY: Caller guarantees valid pointers and safety invariants.
        unsafe { riscv64::arch_copy_to_user_capture_faults(dst, src, len) }
    }
    #[cfg(not(target_arch = "riscv64"))]
    {
        let mut fault_va = 0usize;
        let mut fault_flags = 0u32;
        // SAFETY: Foreign function call with valid pointer arguments.
        let status = unsafe {
            cpp_arch_copy_to_user_capture_faults(
                dst,
                src,
                len,
                &raw mut fault_va,
                &raw mut fault_flags,
            )
        };
        capture_faults_result(status, fault_va, fault_flags)
    }
}

/// Sets whether the current thread is allowed to block.
#[inline(always)]
pub fn set_blocking_disallowed(value: bool) {
    unsafe { cpp_arch_set_blocking_disallowed(value) }
}
