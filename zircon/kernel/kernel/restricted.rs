// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Restricted mode kernel control flow implementation.

use core::ptr::NonNull;
use debug::ltracef;
use kalloc::Box;
use zx_status::Status;
use zx_types::{
    zx_exception_report_t, zx_restricted_exception_t, zx_restricted_reason_t, zx_restricted_state_t,
};

use crate::arch_rs::{Iframe, SyscallRegs};
use crate::kernel::restricted_state::RestrictedState;
use crate::ktrace_rs;
use crate::user_copy::UserOutPtr;

const LOCAL_TRACE: u32 = 0;

/// Leave restricted mode on the current thread and return to normal mode.
///
/// Disables restricted mode flags, saves register state from `restricted_state_source` using `save_fn`,
/// switches the address space back to normal mode, and vectors into normal user space via `enter_full`.
pub fn restricted_leave<T>(
    restricted_state_source: &T,
    save_fn: fn(&mut zx_restricted_state_t, &T),
    reason: zx_restricted_reason_t,
) -> ! {
    ltracef!("regs {:p}\n", restricted_state_source as *const T);

    debug_assert!(crate::arch_rs::ints_disabled());

    let rs_ptr = crate::kernel::thread::current_restricted_state();
    let mut rs_ptr =
        NonNull::new(rs_ptr).expect("restricted_leave called without bound RestrictedState");
    // SAFETY: rs_ptr is guaranteed non-null and valid for current thread.
    let rs = unsafe { rs_ptr.as_mut() };

    debug_assert!(rs.in_restricted());
    debug_assert!(crate::arch_rs::is_user_accessible(rs.vector_ptr()));

    rs.set_in_restricted(false);
    crate::arch_rs::set_restricted_flag(false);

    let state = rs.state_mut();
    save_fn(state, restricted_state_source);

    ltracef!(
        "returning to normal mode at vector {:#x}, context {:#x}\n",
        rs.vector_ptr(),
        rs.context()
    );

    crate::vm::vmm::set_active_aspace_normal();

    ktrace_rs::duration_end!("kernel:restricted", "restricted mode", "reason" => reason);

    crate::arch_rs::enter_full(rs.arch_normal_state(), rs.vector_ptr(), rs.context(), reason);
}

/// Leave restricted mode from an interrupt or exception frame (`Iframe`).
pub fn restricted_leave_iframe(iframe: &Iframe, reason: zx_restricted_reason_t) -> ! {
    restricted_leave(iframe, crate::arch_rs::save_restricted_iframe_state, reason);
}

/// Leave restricted mode from a syscall register frame (`SyscallRegs`).
pub fn restricted_leave_syscall(regs: &SyscallRegs, reason: zx_restricted_reason_t) -> ! {
    restricted_leave(regs, crate::arch_rs::save_restricted_syscall_state, reason);
}

/// Enter restricted mode on the current thread using the specified vector table pointer and context.
///
/// # Errors
/// - `Status::INVALID_ARGS`: `vector_table_ptr` is not a valid user-accessible address.
/// - `Status::BAD_STATE`: No `RestrictedState` is bound to the current thread, or state is invalid.
/// - `Status::INTERRUPTED_RETRY`: Current thread has pending signals that must be processed.
pub fn restricted_enter(vector_table_ptr: usize, context: usize) -> Result<(), Status> {
    ltracef!("vector {:#x} context {:#x}\n", vector_table_ptr, context);

    if !crate::arch_rs::is_user_accessible(vector_table_ptr) {
        return Err(Status::INVALID_ARGS);
    }

    let rs_ptr = crate::kernel::thread::current_restricted_state();
    if rs_ptr.is_null() {
        return Err(Status::BAD_STATE);
    }
    // SAFETY: rs_ptr was verified non-null and is owned by current thread.
    let rs = unsafe { &mut *rs_ptr };
    debug_assert!(!rs.in_restricted());

    let state_buffer = rs.state_ptr();
    if state_buffer.is_null() {
        return Err(Status::BAD_STATE);
    }

    // Copy the state out of the state buffer and into an automatic variable.
    // User mode may have a mapping of the state buffer so it's critical that we make a copy
    // and then validate the copy before using the copy to avoid a ToCToU vulnerability.
    // Use a compiler barrier to ensure the copy actually happens.
    // SAFETY: state_buffer points to a valid mapped zx_restricted_state_t.
    let state: zx_restricted_state_t = unsafe { *state_buffer };
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);

    if LOCAL_TRACE != 0 {
        crate::arch_rs::dump(&state);
    }

    crate::arch_rs::validate_state_pre_restricted_entry(&state)?;

    crate::arch_rs::disable_ints();
    if crate::kernel::thread::current_is_signaled() {
        crate::arch_rs::enable_ints();
        return Err(Status::INTERRUPTED_RETRY);
    }

    crate::arch_rs::save_state_pre_restricted_entry(rs.arch_normal_state_mut());

    rs.set_vector_ptr(vector_table_ptr);
    rs.set_context(context);

    if crate::kernel::thread::current_check_for_restricted_kick() {
        ktrace_rs::duration_end!("kernel:syscall", "restricted_enter");
        crate::arch_rs::enter_full(
            rs.arch_normal_state(),
            vector_table_ptr,
            context,
            zx_types::ZX_RESTRICTED_REASON_KICK,
        );
    }

    crate::vm::vmm::set_active_aspace_restricted();
    rs.set_in_restricted(true);
    crate::arch_rs::set_restricted_flag(true);

    ktrace_rs::duration_end!("kernel:syscall", "restricted_enter");
    ktrace_rs::duration_begin!("kernel:restricted", "restricted mode");

    crate::arch_rs::enter_restricted(&state);
}

/// Sets or clears the current thread's restricted mode state.
pub fn thread_current_set_restricted_state(state: Option<Box<RestrictedState>>) {
    let raw_ptr = match state {
        Some(s) => Box::into_raw(s),
        None => core::ptr::null_mut(),
    };
    // SAFETY: thread_current_set_restricted_state takes ownership of the raw pointer.
    unsafe { crate::kernel::thread::current_set_restricted_state(raw_ptr) };
}

/// Redirect a synchronous exception raised while in restricted mode to normal mode handling.
pub fn redirect_restricted_exception_to_normal_mode(
    rs: &mut RestrictedState,
    report: &zx_exception_report_t,
) {
    debug_assert!(rs.in_restricted());
    let state = rs.state_mut();

    ktrace_rs::duration_end!("kernel:restricted", "restricted mode", "reason" => zx_types::ZX_RESTRICTED_REASON_EXCEPTION);

    crate::arch_rs::save_restricted_exception_state(state);

    rs.set_in_restricted(false);
    crate::arch_rs::set_restricted_flag(false);

    crate::vm::vmm::set_active_aspace_normal();

    let mut reason = zx_types::ZX_RESTRICTED_REASON_EXCEPTION;
    let exception_report_ptr = rs.exception_report_ptr();
    if let Some(report_ptr) = exception_report_ptr {
        let user_out = UserOutPtr::<u8>::new(report_ptr.as_ptr().cast::<u8>());
        // SAFETY: report points to a valid zx_exception_report_t instance of size_of::<zx_exception_report_t>() bytes.
        let report_bytes = unsafe {
            core::slice::from_raw_parts(
                (report as *const zx_exception_report_t).cast::<u8>(),
                core::mem::size_of::<zx_exception_report_t>(),
            )
        };
        if user_out.copy_slice_to_user(report_bytes).is_err() {
            reason = zx_types::ZX_RESTRICTED_REASON_EXCEPTION_LOST;
        }
    } else {
        // SAFETY: Casts mapped state buffer to zx_restricted_exception_t and writes report.
        let restricted_exception = unsafe {
            rs.state_ptr_as::<zx_restricted_exception_t>()
                .as_mut()
                .expect("state_ptr must be non-null")
        };
        restricted_exception.exception = *report;
    }

    crate::arch_rs::redirect_restricted_exception_to_normal(
        rs.arch_normal_state(),
        rs.vector_ptr(),
        rs.context(),
        reason,
    );
}

// FFI Export Wrappers

/// Entry point called directly from hardware exception / syscall assembly vectors.
///
/// # Safety
/// Caller must ensure `regs` is a valid pointer to saved syscall registers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn syscall_from_restricted(regs: *const SyscallRegs) -> ! {
    // SAFETY: regs is guaranteed non-null and valid by assembly caller.
    let regs_ref = unsafe { &*regs };
    restricted_leave_syscall(regs_ref, zx_types::ZX_RESTRICTED_REASON_SYSCALL);
}

/// FFI trampoline for `redirect_restricted_exception_to_normal_mode`.
///
/// # Safety
/// Caller must pass valid references to `RestrictedState` and `zx_exception_report_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_redirect_restricted_exception_to_normal_mode(
    rs: &mut RestrictedState,
    report: &zx_exception_report_t,
) {
    redirect_restricted_exception_to_normal_mode(rs, report);
}

/// FFI trampoline for `restricted_leave_iframe`.
///
/// # Safety
/// Caller must pass valid references to `Iframe`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_restricted_leave_iframe(
    iframe: &Iframe,
    reason: zx_restricted_reason_t,
) -> ! {
    restricted_leave_iframe(iframe, reason);
}

/// FFI trampoline for `restricted_leave_syscall`.
///
/// # Safety
/// Caller must pass valid references to `SyscallRegs`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_restricted_leave_syscall(
    regs: &SyscallRegs,
    reason: zx_restricted_reason_t,
) -> ! {
    restricted_leave_syscall(regs, reason);
}

/// Restricted mode kernel unit tests.
#[cfg(ktest)]
#[unittest::suite(name = "restricted_tests")]
mod tests {
    use super::restricted_enter;
    use zx_status::Status;

    /// Verifies that restricted_enter returns INVALID_ARGS for non-user-accessible vector table addresses.
    #[test]
    fn test_restricted_enter_invalid_args() {
        let invalid_vector = 0xffff_8000_0000_0000usize;
        let res = restricted_enter(invalid_vector, 0);
        unittest::expect_true!(res.unwrap_err() == Status::INVALID_ARGS);
    }

    /// Verifies that restricted_enter returns BAD_STATE when no RestrictedState is bound to the thread.
    #[test]
    fn test_restricted_enter_bad_state_no_restricted_state() {
        // Ensure no RestrictedState is bound to the current thread during this test.
        let prev_rs_ptr = crate::kernel::thread::current_restricted_state();
        unsafe { crate::kernel::thread::current_set_restricted_state(core::ptr::null_mut()) };

        let res = restricted_enter(0x1000, 0);
        unittest::expect_true!(res.unwrap_err() == Status::BAD_STATE);

        // Restore previous RestrictedState pointer.
        unsafe { crate::kernel::thread::current_set_restricted_state(prev_rs_ptr) };
    }
}
