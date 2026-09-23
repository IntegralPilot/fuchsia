// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use static_assertions::const_assert_eq;

const NUM_FP_REGISTERS: usize = 32;
pub const NUM_V_REGISTERS: usize = 32;

// Currently only VLEN=128 is supported.
pub const VLEN: usize = 128;

#[derive(Copy, Clone, Default, PartialEq, Debug)]
#[repr(C)]
pub struct RiscvVectorCsrs {
    pub vcsr: u64,
    pub vstart: u64,
    pub vl: u64,
    pub vtype: u64,
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct State {
    // Floating-point registers from the F and D extensions.
    pub fp_registers: [u64; NUM_FP_REGISTERS],
    pub fcsr: u32,

    // V registers size depends on the CPU and is not defined in compile time. Allocate the buffer
    // for these registers on the heap.
    pub v_registers: [u128; NUM_V_REGISTERS],
    pub vcsrs: RiscvVectorCsrs,
}

const_assert_eq!(std::mem::align_of::<u128>(), VLEN / 8);
const_assert_eq!(std::mem::size_of::<u128>(), VLEN / 8);

// Ensure ABI compatibility with assembly routines in `riscv64_asm.S`.
static_assertions::assert_eq_align!(State, u128); // Alignment should be at least 16 for vector ops
const_assert_eq!(std::mem::offset_of!(State, fp_registers), 0);
// LINT.IfChange(riscv64_state_offsets)
const_assert_eq!(std::mem::offset_of!(State, fcsr), 256);
const_assert_eq!(std::mem::offset_of!(State, v_registers), 272);
const_assert_eq!(std::mem::offset_of!(State, vcsrs), 784);
// LINT.ThenChange(riscv64_asm.S:riscv64_state_offsets)

static_assertions::assert_eq_align!(RiscvVectorCsrs, u64);
// LINT.IfChange(riscv64_vector_csrs_offsets)
const_assert_eq!(std::mem::offset_of!(RiscvVectorCsrs, vcsr), 0);
const_assert_eq!(std::mem::offset_of!(RiscvVectorCsrs, vstart), 8);
const_assert_eq!(std::mem::offset_of!(RiscvVectorCsrs, vl), 16);
const_assert_eq!(std::mem::offset_of!(RiscvVectorCsrs, vtype), 24);
// LINT.ThenChange(riscv64_asm.S:riscv64_vector_csrs_offsets)

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
    fn save_restore_registers() {
        let mut state = crate::ExtendedPstateState::default();
        let mut pstate_ptr_struct = crate::ExtendedPstatePointer { extended_pstate: &mut state };
        let pstate_ptr = &mut pstate_ptr_struct as *mut crate::ExtendedPstatePointer;

        let mut restored_state = State::default();
        let buffer_ptr = &mut restored_state as *mut State as *mut u8;

        let base_sentinel_f: u64 = 0x01234567_89ABCDEF;
        let mut sentinels_f = [0u64; NUM_FP_REGISTERS];
        for i in 0..NUM_FP_REGISTERS {
            sentinels_f[i] = base_sentinel_f + i as u64;
        }

        let base_sentinel_v: u128 = 0x01234567_89ABCDEF_FEDCBA98_76543210_u128;
        let mut sentinels_v = [0u128; NUM_V_REGISTERS];
        for i in 0..NUM_V_REGISTERS {
            sentinels_v[i] = base_sentinel_v + i as u128;
        }

        // SAFETY: all memory accesses are to mutable variables on the stack and all clobbers are
        // specified.
        unsafe {
            asm!(
                // 1. Load sentinels into F registers
                "fld f0, 0({sentinels_f})",
                "fld f1, 8({sentinels_f})",
                "fld f2, 16({sentinels_f})",
                "fld f3, 24({sentinels_f})",
                "fld f4, 32({sentinels_f})",
                "fld f5, 40({sentinels_f})",
                "fld f6, 48({sentinels_f})",
                "fld f7, 56({sentinels_f})",
                "fld f8, 64({sentinels_f})",
                "fld f9, 72({sentinels_f})",
                "fld f10, 80({sentinels_f})",
                "fld f11, 88({sentinels_f})",
                "fld f12, 96({sentinels_f})",
                "fld f13, 104({sentinels_f})",
                "fld f14, 112({sentinels_f})",
                "fld f15, 120({sentinels_f})",
                "fld f16, 128({sentinels_f})",
                "fld f17, 136({sentinels_f})",
                "fld f18, 144({sentinels_f})",
                "fld f19, 152({sentinels_f})",
                "fld f20, 160({sentinels_f})",
                "fld f21, 168({sentinels_f})",
                "fld f22, 176({sentinels_f})",
                "fld f23, 184({sentinels_f})",
                "fld f24, 192({sentinels_f})",
                "fld f25, 200({sentinels_f})",
                "fld f26, 208({sentinels_f})",
                "fld f27, 216({sentinels_f})",
                "fld f28, 224({sentinels_f})",
                "fld f29, 232({sentinels_f})",
                "fld f30, 240({sentinels_f})",
                "fld f31, 248({sentinels_f})",

                // 2. Load sentinels into V registers
                "vl8r.v v0, ({sentinels_v})",
                "addi t2, {sentinels_v}, 128",
                "vl8r.v v8, (t2)",
                "addi t2, t2, 128",
                "vl8r.v v16, (t2)",
                "addi t2, t2, 128",
                "vl8r.v v24, (t2)",

                // 3. Call save routine
                "mv a0, {pstate_ptr}",
                "call {save_fn}",

                // 4. Zero registers
                "fmv.d.x f0, zero",
                "fmv.d.x f1, zero",
                "fmv.d.x f2, zero",
                "fmv.d.x f3, zero",
                "fmv.d.x f4, zero",
                "fmv.d.x f5, zero",
                "fmv.d.x f6, zero",
                "fmv.d.x f7, zero",
                "fmv.d.x f8, zero",
                "fmv.d.x f9, zero",
                "fmv.d.x f10, zero",
                "fmv.d.x f11, zero",
                "fmv.d.x f12, zero",
                "fmv.d.x f13, zero",
                "fmv.d.x f14, zero",
                "fmv.d.x f15, zero",
                "fmv.d.x f16, zero",
                "fmv.d.x f17, zero",
                "fmv.d.x f18, zero",
                "fmv.d.x f19, zero",
                "fmv.d.x f20, zero",
                "fmv.d.x f21, zero",
                "fmv.d.x f22, zero",
                "fmv.d.x f23, zero",
                "fmv.d.x f24, zero",
                "fmv.d.x f25, zero",
                "fmv.d.x f26, zero",
                "fmv.d.x f27, zero",
                "fmv.d.x f28, zero",
                "fmv.d.x f29, zero",
                "fmv.d.x f30, zero",
                "fmv.d.x f31, zero",

                "vl8r.v v0, ({zero_v})",
                "addi t2, {zero_v}, 128",
                "vl8r.v v8, (t2)",
                "addi t2, t2, 128",
                "vl8r.v v16, (t2)",
                "addi t2, t2, 128",
                "vl8r.v v24, (t2)",

                // 5. Call restore routine
                "mv a0, {pstate_ptr}",
                "call {restore_fn}",

                // 6. Save registers to output buffer
                "fsd f0, 0({buffer_ptr_out})",
                "fsd f1, 8({buffer_ptr_out})",
                "fsd f2, 16({buffer_ptr_out})",
                "fsd f3, 24({buffer_ptr_out})",
                "fsd f4, 32({buffer_ptr_out})",
                "fsd f5, 40({buffer_ptr_out})",
                "fsd f6, 48({buffer_ptr_out})",
                "fsd f7, 56({buffer_ptr_out})",
                "fsd f8, 64({buffer_ptr_out})",
                "fsd f9, 72({buffer_ptr_out})",
                "fsd f10, 80({buffer_ptr_out})",
                "fsd f11, 88({buffer_ptr_out})",
                "fsd f12, 96({buffer_ptr_out})",
                "fsd f13, 104({buffer_ptr_out})",
                "fsd f14, 112({buffer_ptr_out})",
                "fsd f15, 120({buffer_ptr_out})",
                "fsd f16, 128({buffer_ptr_out})",
                "fsd f17, 136({buffer_ptr_out})",
                "fsd f18, 144({buffer_ptr_out})",
                "fsd f19, 152({buffer_ptr_out})",
                "fsd f20, 160({buffer_ptr_out})",
                "fsd f21, 168({buffer_ptr_out})",
                "fsd f22, 176({buffer_ptr_out})",
                "fsd f23, 184({buffer_ptr_out})",
                "fsd f24, 192({buffer_ptr_out})",
                "fsd f25, 200({buffer_ptr_out})",
                "fsd f26, 208({buffer_ptr_out})",
                "fsd f27, 216({buffer_ptr_out})",
                "fsd f28, 224({buffer_ptr_out})",
                "fsd f29, 232({buffer_ptr_out})",
                "fsd f30, 240({buffer_ptr_out})",
                "fsd f31, 248({buffer_ptr_out})",

                "addi t2, {buffer_ptr_out}, 272", // Offset of v_registers in State
                "vs8r.v v0, (t2)",
                "addi t2, t2, 128",
                "vs8r.v v8, (t2)",
                "addi t2, t2, 128",
                "vs8r.v v16, (t2)",
                "addi t2, t2, 128",
                "vs8r.v v24, (t2)",

                sentinels_f = in(reg) &sentinels_f,
                sentinels_v = in(reg) &sentinels_v,
                zero_v = in(reg) &[0u128; NUM_V_REGISTERS],
                pstate_ptr = in(reg) pstate_ptr,
                buffer_ptr_out = in(reg) buffer_ptr,
                save_fn = sym save_extended_pstate,
                restore_fn = sym restore_extended_pstate,
                clobber_abi("C"),
                out("f0") _, out("f1") _, out("f2") _, out("f3") _,
                out("f4") _, out("f5") _, out("f6") _, out("f7") _,
                out("f8") _, out("f9") _, out("f10") _, out("f11") _,
                out("f12") _, out("f13") _, out("f14") _, out("f15") _,
                out("f16") _, out("f17") _, out("f18") _, out("f19") _,
                out("f20") _, out("f21") _, out("f22") _, out("f23") _,
                out("f24") _, out("f25") _, out("f26") _, out("f27") _,
                out("f28") _, out("f29") _, out("f30") _, out("f31") _,
            );
        }

        // Assertions
        let state_val = state.get_riscv64_state();
        for i in 0..NUM_FP_REGISTERS {
            assert_eq!(
                state_val.fp_registers[i], sentinels_f[i],
                "state.fp_registers[{}] mismatch",
                i
            );
        }
        for i in 0..NUM_V_REGISTERS {
            assert_eq!(
                state_val.v_registers[i], sentinels_v[i],
                "state.v_registers[{}] mismatch",
                i
            );
        }

        for i in 0..NUM_FP_REGISTERS {
            assert_eq!(
                restored_state.fp_registers[i], sentinels_f[i],
                "restored_state.fp_registers[{}] mismatch",
                i
            );
        }
        for i in 0..NUM_V_REGISTERS {
            assert_eq!(
                restored_state.v_registers[i], sentinels_v[i],
                "restored_state.v_registers[{}] mismatch",
                i
            );
        }
    }
}
