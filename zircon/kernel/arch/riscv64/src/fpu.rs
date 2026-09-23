// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RISC-V 64 floating point unit (FPU) management.

use super::arch::{
    RISCV64_CSR_SSTATUS, RISCV64_CSR_SSTATUS_FS_CLEAN, RISCV64_CSR_SSTATUS_FS_INITIAL,
    RISCV64_CSR_SSTATUS_FS_MASK, riscv64_csr_clear, riscv64_csr_set,
};
use super::thread::{Riscv64FpuState, thread_arch};
use libarch::riscv64::{ExtensionStatus, SSTATUS};

unsafe extern "C" {
    /// Low-level assembly routine to zero FPU registers and reset status to Initial.
    pub fn riscv64_fpu_zero();
    /// Low-level assembly routine to save 32 64-bit float registers and fcsr.
    pub fn riscv64_fpu_save(state: *mut Riscv64FpuState);
    /// Low-level assembly routine to restore 32 64-bit float registers and fcsr.
    pub fn riscv64_fpu_restore(state: *const Riscv64FpuState);
}

/// Read the current CPU FPU status from `sstatus.fs`.
#[inline(always)]
pub fn riscv64_fpu_status() -> ExtensionStatus {
    SSTATUS.read().fs()
}

/// Save the current on-cpu fpu state to the thread. Takes as an argument
/// the current HW status in the sstatus register.
///
/// # Safety
/// Caller guarantees `thread` is a valid Thread pointer.
pub unsafe fn riscv64_thread_fpu_save(thread: *mut core::ffi::c_void, status: ExtensionStatus) {
    debug_assert!(!thread.is_null());
    match status {
        ExtensionStatus::Dirty => {
            // The hardware state is dirty, save the old state.
            // SAFETY: Caller guarantees thread is valid; saves FPU hardware state.
            let arch = unsafe { &mut *thread_arch(thread) };
            // SAFETY: `riscv64_fpu_save` writes the FP register file into
            // `arch.fpu_state`, which is uniquely borrowed here and large enough.
            unsafe { riscv64_fpu_save(core::ptr::addr_of_mut!(arch.fpu_state)) };

            // Record that this thread has modified the state and will need to restore it
            // from now on out on every context switch.
            arch.fpu_dirty = true;
        }
        // These three states means the thread didn't modify the state, so do not
        // write anything back.
        ExtensionStatus::Initial => {
            // The old thread has the initial zeroed state.
        }
        ExtensionStatus::Clean => {
            // The old thread has some valid state loaded, but it didn't modify it.
        }
        ExtensionStatus::Off => {
            // We currently leave the fpu on all the time, so we should never get this state.
            // If lazy FPU load is implemented, this will be a valid state if the thread has
            // never trapped.
            panic!(
                "riscv context switch: FPU was disabled during context switch: sstatus.fs {:?}",
                status
            );
        }
    }
}

/// Restores the fpu state to hardware from the thread passed in. status should hold what is
/// in the sstatus.fs bits as an optimization to avoid re-reading sstatus any more than necessary.
///
/// # Safety
/// Caller guarantees `thread` is a valid Thread pointer.
pub unsafe fn riscv64_thread_fpu_restore(
    thread: *const core::ffi::c_void,
    status: ExtensionStatus,
) {
    debug_assert!(!thread.is_null());
    // SAFETY: Caller guarantees thread is valid; restores FPU registers or resets to initial.
    let arch = unsafe { &*thread_arch(thread as *mut _) };
    if arch.fpu_dirty {
        // Restore the state from the new thread
        // SAFETY: `riscv64_fpu_restore` loads the FP register file from
        // `arch.fpu_state`, which belongs to the live arch state borrowed above, and
        // the CSR updates touch only the FS field of this hart's `sstatus`.
        unsafe {
            riscv64_fpu_restore(core::ptr::addr_of!(arch.fpu_state));

            // Set the fpu hardware state to clean.  These stay as csrc/csrs rather
            // than an SSTATUS read-modify-write so that the bits hardware owns -- fs
            // itself, and the derived sd -- cannot be clobbered in between.
            riscv64_csr_clear::<RISCV64_CSR_SSTATUS>(RISCV64_CSR_SSTATUS_FS_MASK);
            riscv64_csr_set::<RISCV64_CSR_SSTATUS>(RISCV64_CSR_SSTATUS_FS_CLEAN);
        }
    } else if status != ExtensionStatus::Initial {
        // Zero the state of the fpu unit if it currently is known to have something other
        // than the initial state loaded.
        // SAFETY: `riscv64_fpu_zero` writes only the FP register file, and the CSR
        // updates touch only the FS field of this hart's `sstatus`.
        unsafe {
            riscv64_fpu_zero();

            // Set the fpu hardware state to initial
            riscv64_csr_clear::<RISCV64_CSR_SSTATUS>(RISCV64_CSR_SSTATUS_FS_MASK);
            riscv64_csr_set::<RISCV64_CSR_SSTATUS>(RISCV64_CSR_SSTATUS_FS_INITIAL);
        }
    } else {
        // thread does not have dirty state and fpu state == INITIAL
        // The old fpu hardware should have the initial state here and we didn't reset it
        // so we should still be in the initial state.
        debug_assert_eq!(status, ExtensionStatus::Initial);
    }
}
