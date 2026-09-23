// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Architectural debugger register accessors and control for RISC-V 64.

use super::restricted::Iframe;
use super::thread::GeneralRegsSource;
use zx_status::Status;
use zx_types::zx_thread_state_single_step_t;

/// Opaque zero-sized debug registers representation for RISC-V 64.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct zx_thread_state_debug_regs_t {
    pub unused: u32,
}

/// Get the instruction pointer from the given general registers source.
///
/// # Safety
/// `gregs` must point to a live `Iframe` that outlives the call. riscv64 only
/// ever saves general registers into an iframe, so `source` must be
/// `GeneralRegsSource::Iframe`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_get_instruction_pointer(
    source: GeneralRegsSource,
    gregs: *const core::ffi::c_void,
) -> usize {
    debug_assert!(source == GeneralRegsSource::Iframe);
    // SAFETY: the caller guarantees `gregs` points at a live `Iframe`; the
    // assertion above pins down which representation that is.
    let iframe = unsafe { &*(gregs as *const Iframe) };
    iframe.regs.pc as usize
}

/// Set the return instruction pointer in the given general registers source.
///
/// # Safety
/// `gregs` must point to a live, uniquely borrowed `Iframe` that outlives the
/// call, and `source` must be `GeneralRegsSource::Iframe`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_set_return_instruction_pointer(
    source: GeneralRegsSource,
    gregs: *mut core::ffi::c_void,
    ip: usize,
) -> Result<(), Status> {
    debug_assert!(source == GeneralRegsSource::Iframe);
    // SAFETY: the caller guarantees `gregs` points at a live `Iframe` that
    // nothing else is holding a reference to for the duration of the call.
    let iframe = unsafe { &mut *(gregs as *mut Iframe) };
    iframe.regs.pc = ip as u64;
    Ok(())
}

/// Hardware breakpoint count (0 on current riscv64).
#[unsafe(no_mangle)]
pub extern "C" fn arch_get_hw_breakpoint_count() -> u8 {
    0
}

/// Hardware watchpoint count (0 on current riscv64).
#[unsafe(no_mangle)]
pub extern "C" fn arch_get_hw_watchpoint_count() -> u8 {
    0
}

/// Single step debugging is currently unsupported on riscv64.
///
/// # Safety
/// Nothing: the arguments are never dereferenced. The function is `unsafe` only
/// to match the signature the other architectures export.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_get_single_step(
    _thread: *mut core::ffi::c_void,
    _out: *mut zx_thread_state_single_step_t,
) -> Status {
    Status::NOT_SUPPORTED
}

/// Single step debugging is currently unsupported on riscv64.
///
/// # Safety
/// Nothing: the arguments are never dereferenced. The function is `unsafe` only
/// to match the signature the other architectures export.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_set_single_step(
    _thread: *mut core::ffi::c_void,
    _in: *const zx_thread_state_single_step_t,
) -> Status {
    Status::NOT_SUPPORTED
}

/// Debug registers are zero-sized on riscv64.
///
/// # Safety
/// Nothing: the arguments are never dereferenced. The function is `unsafe` only
/// to match the signature the other architectures export.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_get_debug_regs(
    _thread: *mut core::ffi::c_void,
    _out: *mut zx_thread_state_debug_regs_t,
) -> Result<(), Status> {
    Ok(())
}

/// Debug registers are zero-sized on riscv64.
///
/// # Safety
/// Nothing: the arguments are never dereferenced. The function is `unsafe` only
/// to match the signature the other architectures export.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_set_debug_regs(
    _thread: *mut core::ffi::c_void,
    _in: *const zx_thread_state_debug_regs_t,
) -> Result<(), Status> {
    Ok(())
}
