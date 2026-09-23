// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RISC-V 64 Vector (V) extension state and register management.

use super::arch::{
    RISCV64_CSR_SSTATUS, RISCV64_CSR_SSTATUS_VS_CLEAN, RISCV64_CSR_SSTATUS_VS_INITIAL,
    RISCV64_CSR_SSTATUS_VS_MASK, riscv64_csr_clear, riscv64_csr_set,
};
use super::feature;
use super::thread::{Riscv64VectorState, thread_arch};
use libarch::riscv64::{ExtensionStatus, SSTATUS, VectorType};

unsafe extern "C" {
    /// Low-level assembly routine to zero vector registers and reset status to Initial.
    pub fn riscv64_vector_zero();
    /// Low-level assembly routine to save 32 vector registers, vcsr, vstart, vl, and vtype.
    pub fn riscv64_vector_save(state: *mut Riscv64VectorState);
    /// Low-level assembly routine to restore 32 vector registers, vcsr, vstart, vl, and vtype.
    pub fn riscv64_vector_restore(state: *const Riscv64VectorState);
}

/// Read the current CPU Vector status from `sstatus.vs`.
#[inline(always)]
pub fn riscv64_vector_status() -> ExtensionStatus {
    SSTATUS.read().vs()
}

/// Compute maximum vector length `VLMAX` in elements for a given `vtype` encoding.
fn riscv64_vlmax(vtype: u64) -> Option<u64> {
    let mut value = 8 * feature::vlenb(); // VLEN
    if value == 0 {
        return None;
    }

    let vtype = VectorType::from(vtype);

    // This computes VLEN / SEW
    match vtype.vsew() {
        0b000 => value /= 8,  // SEW = 8
        0b001 => value /= 16, // SEW = 16
        0b010 => value /= 32, // SEW = 32
        0b011 => value /= 64, // SEW = 64
        _ => return None,     // SEW = reserved
    }

    match vtype.vlmul() {
        0b000 => {}           // LMUL = 1
        0b001 => value *= 2,  // LMUL = 2
        0b010 => value *= 4,  // LMUL = 4
        0b011 => value *= 8,  // LMUL = 8
        0b100 => return None, // LMUL = reserved
        0b101 => value /= 8,  // LMUL = 1/8
        0b110 => value /= 4,  // LMUL = 1/4
        0b111 => value /= 2,  // LMUL = 1/2
        _ => return None,
    }

    // If the value is now zero, we had a divisor that was too large and thus an
    // invalid (SEW, LMUL) pair.
    if value == 0 { None } else { Some(value) }
}

/// Save the current on-cpu vector state to the thread. Takes as an argument
/// the current HW status in the sstatus register.
///
/// # Safety
/// Caller guarantees `thread` is a valid Thread pointer.
pub unsafe fn riscv64_thread_vector_save(thread: *mut core::ffi::c_void, status: ExtensionStatus) {
    debug_assert!(!thread.is_null());
    debug_assert!(feature::has_vector());

    match status {
        ExtensionStatus::Dirty => {
            // The hardware state is dirty, save the old state.
            // SAFETY: Caller guarantees thread is valid; saves Vector hardware state.
            let arch = unsafe { &mut *thread_arch(thread) };
            // SAFETY: `riscv64_vector_save` writes the vector register file into
            // `arch.vector_state`, which is uniquely borrowed here and large enough.
            unsafe { riscv64_vector_save(core::ptr::addr_of_mut!(arch.vector_state)) };

            // Record that this thread has modified the state and will need to restore it
            // from now on out on every context switch.
            arch.vector_dirty = true;
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
            // We currently leave vector enabled on all the time, so we should never
            // get this state.
            panic!(
                "riscv context switch: Vector registers were disabled during context switch: sstatus.vs {:?}",
                status
            );
        }
    }
}

/// Restores the vector state to hardware from the thread passed in. status should hold what
/// is in the sstatus.vs bits as an optimization to avoid re-reading sstatus any more
/// than necessary.
///
/// # Safety
/// Caller guarantees `thread` is a valid Thread pointer.
pub unsafe fn riscv64_thread_vector_restore(
    thread: *const core::ffi::c_void,
    status: ExtensionStatus,
) {
    debug_assert!(!thread.is_null());
    debug_assert!(feature::has_vector());

    // SAFETY: Caller guarantees thread is valid; restores Vector registers or resets to initial.
    let arch = unsafe { &*thread_arch(thread as *mut _) };
    if arch.vector_dirty {
        // Restore the state from the new thread
        // SAFETY: `riscv64_vector_restore` loads the vector register file from
        // `arch.vector_state`, which belongs to the live arch state borrowed above, and
        // the CSR updates touch only the VS field of this hart's `sstatus`.
        unsafe {
            riscv64_vector_restore(core::ptr::addr_of!(arch.vector_state));

            // Set the vector hardware state to clean.  These stay as csrc/csrs rather
            // than an SSTATUS read-modify-write so that the bits hardware owns -- vs
            // itself, and the derived sd -- cannot be clobbered in between.
            riscv64_csr_clear::<RISCV64_CSR_SSTATUS>(RISCV64_CSR_SSTATUS_VS_MASK);
            riscv64_csr_set::<RISCV64_CSR_SSTATUS>(RISCV64_CSR_SSTATUS_VS_CLEAN);
        }
    } else if status != ExtensionStatus::Initial {
        // Zero the state of the vector unit if it currently is known to have something other
        // than the initial state loaded.
        // SAFETY: `riscv64_vector_zero` writes only the vector register file, and the CSR
        // updates touch only the VS field of this hart's `sstatus`.
        unsafe {
            riscv64_vector_zero();

            // Set the vector hardware state to initial
            riscv64_csr_clear::<RISCV64_CSR_SSTATUS>(RISCV64_CSR_SSTATUS_VS_MASK);
            riscv64_csr_set::<RISCV64_CSR_SSTATUS>(RISCV64_CSR_SSTATUS_VS_INITIAL);
        }
    } else {
        // thread does not have dirty state and vector state == INITIAL
        // The old vector hardware should have the initial state here and we didn't reset it
        // so we should still be in the initial state.
        debug_assert_eq!(status, ExtensionStatus::Initial);
    }
}

// C FFI exports

/// Compute VLMAX for C callers, returning (value, has_value).
#[unsafe(no_mangle)]
pub extern "C" fn rust_riscv64_vlmax(vtype: u64, has_value: *mut bool) -> u64 {
    debug_assert!(!has_value.is_null());
    match riscv64_vlmax(vtype) {
        Some(val) => {
            // SAFETY: Caller guarantees pointer validity.
            unsafe { *has_value = true };
            val
        }
        None => {
            // SAFETY: Caller guarantees pointer validity.
            unsafe { *has_value = false };
            0
        }
    }
}

#[cfg(ktest)]
/// Tests for RISC-V 64 VLMAX computation.
#[unittest::suite(name = "riscv64_vector")]
mod tests {
    use super::riscv64_vlmax;
    use unittest::assert_true;

    /// Test VLMAX calculation for valid SEW and LMUL combinations.
    #[test]
    fn test_vlmax_calculation() {
        // vlenb = 16 (VLEN = 128 bits = 16 bytes).
        // If vlenb is 0 in test environment, riscv64_vlmax returns None.
        let vlenb = super::feature::vlenb();
        if vlenb == 16 {
            // SEW = 8 (0b000), LMUL = 1 (0b000) => 128 / 8 * 1 = 16
            assert_true!(riscv64_vlmax(0b000_000) == Some(16));
            // SEW = 16 (0b001), LMUL = 1 (0b000) => 128 / 16 * 1 = 8
            assert_true!(riscv64_vlmax(0b001_000) == Some(8));
            // SEW = 32 (0b010), LMUL = 1 (0b000) => 128 / 32 * 1 = 4
            assert_true!(riscv64_vlmax(0b010_000) == Some(4));
            // SEW = 64 (0b011), LMUL = 1 (0b000) => 128 / 64 * 1 = 2
            assert_true!(riscv64_vlmax(0b011_000) == Some(2));
            // SEW = 32 (0b010), LMUL = 2 (0b001) => 4 * 2 = 8
            assert_true!(riscv64_vlmax(0b010_001) == Some(8));
            // SEW = 32 (0b010), LMUL = 4 (0b010) => 4 * 4 = 16
            assert_true!(riscv64_vlmax(0b010_010) == Some(16));
            // SEW = 32 (0b010), LMUL = 8 (0b011) => 4 * 8 = 32
            assert_true!(riscv64_vlmax(0b010_011) == Some(32));
        }
    }
}
