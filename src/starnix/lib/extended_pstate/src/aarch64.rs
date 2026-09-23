// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use static_assertions::const_assert_eq;

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct State {
    // [arm/v8]: A1.3.1 Execution state
    // 32 registers, 128 bits each
    pub q: [u128; 32],
    // [arm/v8]: A1.5 Advanced SIMD and floating-point support
    pub fpcr: u32,
    pub fpsr: u32,
}

const_assert_eq!(std::mem::size_of::<State>(), 512 + 16);

// Ensure ABI compatibility with assembly routines in `aarch64_asm.S`.
static_assertions::assert_eq_align!(State, u128);
const_assert_eq!(std::mem::offset_of!(State, q), 0);
// LINT.IfChange(aarch64_state_offsets)
const_assert_eq!(std::mem::offset_of!(State, fpcr), 512);
const_assert_eq!(std::mem::offset_of!(State, fpsr), 516);
// LINT.ThenChange(aarch64_asm.S:aarch64_state_offsets)

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct Aarch32State {
    // [arm/v8]: E1.3.1 The SIMD and floating-point register file
    // 16 registers, 128 bits each
    pub q: [u128; 16],

    // AArch32 technically has only 32 bits of user space accessible status/control space, see
    // [arm/v8]: G8.2.55 FPSCR, Floating-Point Status and Control Register.
    // The restricted mode implementation maps these to the fpcr/fpsr registers used by
    // AArch64, so we store those here instead of a single u32.
    pub fpcr: u32,
    pub fpsr: u32,
}

const_assert_eq!(std::mem::size_of::<Aarch32State>(), 256 + 16);

// Ensure ABI compatibility with assembly routines in `aarch64_asm.S`.
static_assertions::assert_eq_align!(Aarch32State, u128);
const_assert_eq!(std::mem::offset_of!(Aarch32State, q), 0);
// LINT.IfChange(aarch32_state_offsets)
const_assert_eq!(std::mem::offset_of!(Aarch32State, fpcr), 256);
const_assert_eq!(std::mem::offset_of!(Aarch32State, fpsr), 260);
// LINT.ThenChange(aarch64_asm.S:aarch32_state_offsets)

// Aarch64 supports aligned and unaligned stores to/from vector registers. Aligned accesses may be
// faster.
const_assert_eq!(std::mem::align_of::<u128>(), 16);

impl Aarch32State {
    pub fn reset(&mut self) {
        *self = Default::default();
    }
}

impl State {
    pub fn reset(&mut self) {
        *self = Default::default();
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::*;
    use core::arch::asm;

    #[fuchsia::test]
    fn test_save_restore_64() {
        let mut state = crate::ExtendedPstateState::default();
        let mut pstate_ptr_struct = crate::ExtendedPstatePointer { extended_pstate: &mut state };
        let pstate_ptr = &mut pstate_ptr_struct as *mut crate::ExtendedPstatePointer;

        let mut restored_state = State::default();
        let buffer_ptr = &mut restored_state as *mut State as *mut u8;

        let sentinel_q: u128 = 0xDEADBEEF_DEADBEEF_DEADBEEF_DEADBEEF_u128;
        let sentinel_fpcr: u64 = 0x01000000; // FZ bit
        let sentinel_fpsr: u64 = 0;

        let sentinels_q = [sentinel_q; 32];

        // SAFETY: all memory accesses are to mutable variables on the stack and all clobbers are
        // specified.
        unsafe {
            asm!(
                // 1. Load sentinels into registers
                "ldp q0, q1, [{sentinels_q}]",
                "ldp q2, q3, [{sentinels_q}, #32]",
                "ldp q4, q5, [{sentinels_q}, #64]",
                "ldp q6, q7, [{sentinels_q}, #96]",
                "ldp q8, q9, [{sentinels_q}, #128]",
                "ldp q10, q11, [{sentinels_q}, #160]",
                "ldp q12, q13, [{sentinels_q}, #192]",
                "ldp q14, q15, [{sentinels_q}, #224]",
                "ldp q16, q17, [{sentinels_q}, #256]",
                "ldp q18, q19, [{sentinels_q}, #288]",
                "ldp q20, q21, [{sentinels_q}, #320]",
                "ldp q22, q23, [{sentinels_q}, #352]",
                "ldp q24, q25, [{sentinels_q}, #384]",
                "ldp q26, q27, [{sentinels_q}, #416]",
                "ldp q28, q29, [{sentinels_q}, #448]",
                "ldp q30, q31, [{sentinels_q}, #480]",
                "msr fpcr, {sentinel_fpcr_reg}",
                "msr fpsr, {sentinel_fpsr_reg}",

                // 2. Call save routine
                "mov x0, {pstate_ptr}",
                "bl {save_fn}",

                // 3. Zero registers
                "eor v0.16b, v0.16b, v0.16b",
                "eor v1.16b, v1.16b, v1.16b",
                "eor v2.16b, v2.16b, v2.16b",
                "eor v3.16b, v3.16b, v3.16b",
                "eor v4.16b, v4.16b, v4.16b",
                "eor v5.16b, v5.16b, v5.16b",
                "eor v6.16b, v6.16b, v6.16b",
                "eor v7.16b, v7.16b, v7.16b",
                "eor v8.16b, v8.16b, v8.16b",
                "eor v9.16b, v9.16b, v9.16b",
                "eor v10.16b, v10.16b, v10.16b",
                "eor v11.16b, v11.16b, v11.16b",
                "eor v12.16b, v12.16b, v12.16b",
                "eor v13.16b, v13.16b, v13.16b",
                "eor v14.16b, v14.16b, v14.16b",
                "eor v15.16b, v15.16b, v15.16b",
                "eor v16.16b, v16.16b, v16.16b",
                "eor v17.16b, v17.16b, v17.16b",
                "eor v18.16b, v18.16b, v18.16b",
                "eor v19.16b, v19.16b, v19.16b",
                "eor v20.16b, v20.16b, v20.16b",
                "eor v21.16b, v21.16b, v21.16b",
                "eor v22.16b, v22.16b, v22.16b",
                "eor v23.16b, v23.16b, v23.16b",
                "eor v24.16b, v24.16b, v24.16b",
                "eor v25.16b, v25.16b, v25.16b",
                "eor v26.16b, v26.16b, v26.16b",
                "eor v27.16b, v27.16b, v27.16b",
                "eor v28.16b, v28.16b, v28.16b",
                "eor v29.16b, v29.16b, v29.16b",
                "eor v30.16b, v30.16b, v30.16b",
                "eor v31.16b, v31.16b, v31.16b",
                "msr fpcr, xzr",
                "msr fpsr, xzr",

                // 4. Call restore routine
                "mov x0, {pstate_ptr}",
                "bl {restore_fn}",

                // 5. Save registers to buffer
                "stp q0, q1, [{buffer_ptr_out}]",
                "stp q2, q3, [{buffer_ptr_out}, #32]",
                "stp q4, q5, [{buffer_ptr_out}, #64]",
                "stp q6, q7, [{buffer_ptr_out}, #96]",
                "stp q8, q9, [{buffer_ptr_out}, #128]",
                "stp q10, q11, [{buffer_ptr_out}, #160]",
                "stp q12, q13, [{buffer_ptr_out}, #192]",
                "stp q14, q15, [{buffer_ptr_out}, #224]",
                "stp q16, q17, [{buffer_ptr_out}, #256]",
                "stp q18, q19, [{buffer_ptr_out}, #288]",
                "stp q20, q21, [{buffer_ptr_out}, #320]",
                "stp q22, q23, [{buffer_ptr_out}, #352]",
                "stp q24, q25, [{buffer_ptr_out}, #384]",
                "stp q26, q27, [{buffer_ptr_out}, #416]",
                "stp q28, q29, [{buffer_ptr_out}, #448]",
                "stp q30, q31, [{buffer_ptr_out}, #480]",
                "mrs x2, fpcr",
                "str w2, [{buffer_ptr_out}, #512]",
                "mrs x3, fpsr",
                "str w3, [{buffer_ptr_out}, #516]",

                sentinels_q = in(reg) &sentinels_q,
                sentinel_fpcr_reg = in(reg) sentinel_fpcr,
                sentinel_fpsr_reg = in(reg) sentinel_fpsr,
                pstate_ptr = in(reg) pstate_ptr,
                buffer_ptr_out = in(reg) buffer_ptr,
                save_fn = sym save_extended_pstate,
                restore_fn = sym restore_extended_pstate,
                clobber_abi("C"),
                out("x0") _,
                out("x1") _,
                out("x2") _,
                out("x3") _,
                out("v0") _, out("v1") _, out("v2") _, out("v3") _,
                out("v4") _, out("v5") _, out("v6") _, out("v7") _,
                out("v8") _, out("v9") _, out("v10") _, out("v11") _,
                out("v12") _, out("v13") _, out("v14") _, out("v15") _,
                out("v16") _, out("v17") _, out("v18") _, out("v19") _,
                out("v20") _, out("v21") _, out("v22") _, out("v23") _,
                out("v24") _, out("v25") _, out("v26") _, out("v27") _,
                out("v28") _, out("v29") _, out("v30") _, out("v31") _,
                out("x30") _,
            );
        }

        // Assertions
        let state_val = state.get_arm64_qregs();
        for i in 0..32 {
            assert_eq!(state_val[i], sentinel_q, "state.q[{}] mismatch", i);
        }
        assert_eq!(state.get_arm64_fpcr(), sentinel_fpcr as u32, "state.fpcr mismatch");
        assert_eq!(state.get_arm64_fpsr(), sentinel_fpsr as u32, "state.fpsr mismatch");

        for i in 0..32 {
            assert_eq!(restored_state.q[i], sentinel_q, "restored_state.q[{}] mismatch", i);
        }
        assert_eq!(restored_state.fpcr, sentinel_fpcr as u32, "restored_state.fpcr mismatch");
        assert_eq!(restored_state.fpsr, sentinel_fpsr as u32, "restored_state.fpsr mismatch");
    }

    #[fuchsia::test]
    fn test_save_restore_32() {
        let mut state = crate::ExtendedAarch32PstateState::default();
        let mut pstate_ptr_struct =
            crate::ExtendedPstatePointer { extended_aarch32_pstate: &mut state };
        let pstate_ptr = &mut pstate_ptr_struct as *mut crate::ExtendedPstatePointer;

        let mut restored_state = Aarch32State::default();
        let buffer_ptr = &mut restored_state as *mut Aarch32State as *mut u8;

        let sentinel_q: u128 = 0xDEADBEEF_DEADBEEF_DEADBEEF_DEADBEEF_u128;
        let sentinel_fpcr: u64 = 0x01000000;
        let sentinel_fpsr: u64 = 0;

        let sentinels_q = [sentinel_q; 16];

        // SAFETY: all memory accesses are to mutable variables on the stack and all clobbers are
        // specified.
        unsafe {
            asm!(
                // 1. Load sentinels into registers
                "ldp q0, q1, [{sentinels_q}]",
                "ldp q2, q3, [{sentinels_q}, #32]",
                "ldp q4, q5, [{sentinels_q}, #64]",
                "ldp q6, q7, [{sentinels_q}, #96]",
                "ldp q8, q9, [{sentinels_q}, #128]",
                "ldp q10, q11, [{sentinels_q}, #160]",
                "ldp q12, q13, [{sentinels_q}, #192]",
                "ldp q14, q15, [{sentinels_q}, #224]",
                "msr fpcr, {sentinel_fpcr_reg}",
                "msr fpsr, {sentinel_fpsr_reg}",

                // 2. Call save routine
                "mov x0, {pstate_ptr}",
                "bl {save_fn}",

                // 3. Zero registers
                "eor v0.16b, v0.16b, v0.16b",
                "eor v1.16b, v1.16b, v1.16b",
                "eor v2.16b, v2.16b, v2.16b",
                "eor v3.16b, v3.16b, v3.16b",
                "eor v4.16b, v4.16b, v4.16b",
                "eor v5.16b, v5.16b, v5.16b",
                "eor v6.16b, v6.16b, v6.16b",
                "eor v7.16b, v7.16b, v7.16b",
                "eor v8.16b, v8.16b, v8.16b",
                "eor v9.16b, v9.16b, v9.16b",
                "eor v10.16b, v10.16b, v10.16b",
                "eor v11.16b, v11.16b, v11.16b",
                "eor v12.16b, v12.16b, v12.16b",
                "eor v13.16b, v13.16b, v13.16b",
                "eor v14.16b, v14.16b, v14.16b",
                "eor v15.16b, v15.16b, v15.16b",
                "msr fpcr, xzr",
                "msr fpsr, xzr",

                // 4. Call restore routine
                "mov x0, {pstate_ptr}",
                "bl {restore_fn}",

                // 5. Save registers to buffer
                "stp q0, q1, [{buffer_ptr_out}]",
                "stp q2, q3, [{buffer_ptr_out}, #32]",
                "stp q4, q5, [{buffer_ptr_out}, #64]",
                "stp q6, q7, [{buffer_ptr_out}, #96]",
                "stp q8, q9, [{buffer_ptr_out}, #128]",
                "stp q10, q11, [{buffer_ptr_out}, #160]",
                "stp q12, q13, [{buffer_ptr_out}, #192]",
                "stp q14, q15, [{buffer_ptr_out}, #224]",
                "mrs x2, fpcr",
                "str w2, [{buffer_ptr_out}, #256]",
                "mrs x3, fpsr",
                "str w3, [{buffer_ptr_out}, #260]",

                sentinels_q = in(reg) &sentinels_q,
                sentinel_fpcr_reg = in(reg) sentinel_fpcr,
                sentinel_fpsr_reg = in(reg) sentinel_fpsr,
                pstate_ptr = in(reg) pstate_ptr,
                buffer_ptr_out = in(reg) buffer_ptr,
                save_fn = sym save_extended_aarch32_pstate,
                restore_fn = sym restore_extended_aarch32_pstate,
                clobber_abi("C"),
                out("x0") _,
                out("x1") _,
                out("x2") _,
                out("x3") _,
                out("v0") _, out("v1") _, out("v2") _, out("v3") _,
                out("v4") _, out("v5") _, out("v6") _, out("v7") _,
                out("v8") _, out("v9") _, out("v10") _, out("v11") _,
                out("v12") _, out("v13") _, out("v14") _, out("v15") _,
                out("x30") _,
            );
        }

        // Assertions
        let state_val = state.get_arm32_qregs();
        for i in 0..16 {
            assert_eq!(state_val[i], sentinel_q, "state.q[{}] mismatch", i);
        }
        assert_eq!(state.get_arm32_fpcr(), sentinel_fpcr as u32, "state.fpcr mismatch");
        assert_eq!(state.get_arm32_fpsr(), sentinel_fpsr as u32, "state.fpsr mismatch");

        for i in 0..16 {
            assert_eq!(restored_state.q[i], sentinel_q, "restored_state.q[{}] mismatch", i);
        }
        assert_eq!(restored_state.fpcr, sentinel_fpcr as u32, "restored_state.fpcr mismatch");
        assert_eq!(restored_state.fpsr, sentinel_fpsr as u32, "restored_state.fpsr mismatch");
    }
}
