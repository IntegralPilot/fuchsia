// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use static_assertions::const_assert_eq;
use std::sync::LazyLock;

#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct State {
    pub(crate) buffer: XSaveArea,
    strategy: Strategy,
}

// Ensure ABI compatibility with assembly routines in `x86_64_asm.S`.
static_assertions::assert_eq_align!(State, XSaveArea);
// LINT.IfChange(x86_64_state_offsets)
const_assert_eq!(std::mem::offset_of!(State, buffer), 0);
const_assert_eq!(std::mem::offset_of!(State, strategy), 832);
// LINT.ThenChange(x86_64_asm.S:x86_64_state_offsets)

// Size of the XSAVE area.
pub const XSAVE_AREA_SIZE: usize = 832;

const XSAVE_FEATURE_X87: u64 = 1 << 0;
const XSAVE_FEATURE_SSE: u64 = 1 << 1;
const XSAVE_FEATURE_AVX: u64 = 1 << 2;

// Save FPU, SSE and AVX registers. This matches the set of features supported by Zircon (see
// zircon/kernel/arch/x86/registers.cc ).
pub const SUPPORTED_XSAVE_FEATURES: u64 = XSAVE_FEATURE_X87 | XSAVE_FEATURE_SSE | XSAVE_FEATURE_AVX;

const NUM_XMM_REGS: usize = 16;

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct X87MMXState {
    low: u64,
    high: u64,
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct SSERegister {
    low: u64,
    high: u64,
}

// [intel/vol1] Table 10-2. Format of an FXSAVE Area
#[derive(Clone, Copy)]
#[repr(C)]
struct X86LegacySaveArea {
    fcw: u16,
    fsw: u16,
    ftw: u8,
    _reserved: u8,

    fop: u16,
    fip: u64,
    fdp: u64,

    mxcsr: u32,
    mxcsr_mask: u32,

    st: [X87MMXState; 8],

    xmm: [SSERegister; NUM_XMM_REGS],
}

const_assert_eq!(std::mem::size_of::<X86LegacySaveArea>(), 416);

#[derive(Clone, Copy)]
#[repr(C, align(16))]
struct FXSaveArea {
    x86_legacy_save_area: X86LegacySaveArea,
    _reserved: [u8; 96],
}
const_assert_eq!(std::mem::size_of::<FXSaveArea>(), 512);

impl Default for FXSaveArea {
    fn default() -> Self {
        Self {
            x86_legacy_save_area: X86LegacySaveArea {
                fcw: 0x37f, // All exceptions masked, no exceptions raised.
                fsw: 0,
                // The ftw field stores an abbreviated version where all zero bits match the default.
                // See [intel/vol1] 10.5.1.1 x87 State for details.
                ftw: 0,
                _reserved: Default::default(),
                fop: 0,
                fip: 0,
                fdp: 0,
                mxcsr: 0x3f << 7, // All exceptions masked, no exceptions raised.
                mxcsr_mask: 0,
                st: Default::default(),
                xmm: Default::default(),
            },
            _reserved: [0; 96],
        }
    }
}

#[derive(Clone, Copy)]
#[repr(C, align(64))]
pub(crate) struct XSaveArea {
    fxsave_area: FXSaveArea,
    xsave_header: [u8; 64],
    // High 128 bits of ymm0-15 registers
    avx_state: [u8; 256],
    // TODO: Size of the extended region is dynamic depending on which features are enabled.
    // See [intel/vol1] 13.5 XSAVE-MANAGED STATE
}

const_assert_eq!(std::mem::size_of::<XSaveArea>(), XSAVE_AREA_SIZE);

impl Default for XSaveArea {
    fn default() -> Self {
        Self { fxsave_area: Default::default(), xsave_header: [0; 64], avx_state: [0; 256] }
    }
}

#[derive(PartialEq, Debug, Copy, Clone, PartialOrd)]
#[repr(u32)]
pub enum Strategy {
    // LINT.IfChange(strategy_discriminants)
    XSaveOpt = 0,
    XSave = 1,
    FXSave = 2,
    // LINT.ThenChange(x86_64_asm.S:strategy_discriminants)
}

pub static PREFERRED_STRATEGY: LazyLock<Strategy> = LazyLock::new(|| {
    if is_x86_feature_detected!("xsaveopt") {
        Strategy::XSaveOpt
    } else if is_x86_feature_detected!("xsave") {
        Strategy::XSave
    } else {
        // The FXSave strategy does not preserve the high 128 bits of the YMM
        // register. If we find hardware that requires this, we need to add
        // support for saving and restoring these through load/store
        // instructions with the VEX.256 prefix and remove this assertion.
        // [intel/vol1]: 14.8 ACCESSING YMM REGISTERS
        assert!(!is_x86_feature_detected!("avx"));
        Strategy::FXSave
    }
});

impl State {
    pub fn with_strategy(strategy: Strategy) -> Self {
        Self { buffer: XSaveArea::default(), strategy }
    }

    pub fn reset(&mut self) {
        self.initialize_saved_area()
    }

    fn initialize_saved_area(&mut self) {
        *self = Default::default()
    }

    pub(crate) fn set_xsave_area(&mut self, xsave_area: [u8; XSAVE_AREA_SIZE]) {
        self.buffer = {
            #[allow(
                clippy::undocumented_unsafe_blocks,
                reason = "Force documented unsafe blocks in Starnix"
            )]
            unsafe {
                std::mem::transmute(xsave_area)
            }
        };

        // The tail of the FXSAVE are is unused and is ignored. It may be modified when returning
        // from a signal handler. Reset it to zeros.
        self.buffer.fxsave_area._reserved = [0u8; 96];
    }
}

impl Default for State {
    fn default() -> Self {
        Self { buffer: XSaveArea::default(), strategy: *PREFERRED_STRATEGY }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use core::arch::asm;

    const XMM_REG_SIZE: usize = std::mem::size_of::<u128>();

    #[fuchsia::test]
    fn test_save_restore_x86_64() {
        let mut state = crate::ExtendedPstateState::default();
        let mut pstate_ptr_struct = crate::ExtendedPstatePointer { extended_pstate: &mut state };
        let pstate_ptr = &mut pstate_ptr_struct as *mut crate::ExtendedPstatePointer;

        let mut restored_regs = [0u128; NUM_XMM_REGS];
        let restored_regs_ptr = restored_regs.as_mut_ptr() as *mut u8;

        let base_sentinel: u128 = 0x01234567_89ABCDEF_FEDCBA98_76543210_u128;
        let mut sentinels_xmm = [0u128; NUM_XMM_REGS];
        for i in 0..NUM_XMM_REGS {
            sentinels_xmm[i] = base_sentinel + i as u128;
        }

        // SAFETY: all memory accesses are to mutable variables on the stack and all clobbers are
        // specified.
        unsafe {
            asm!(
                // 1. Load sentinels into registers
                "movdqu xmm0, [{sentinels_xmm}]",
                "movdqu xmm1, [{sentinels_xmm} + 16]",
                "movdqu xmm2, [{sentinels_xmm} + 32]",
                "movdqu xmm3, [{sentinels_xmm} + 48]",
                "movdqu xmm4, [{sentinels_xmm} + 64]",
                "movdqu xmm5, [{sentinels_xmm} + 80]",
                "movdqu xmm6, [{sentinels_xmm} + 96]",
                "movdqu xmm7, [{sentinels_xmm} + 112]",
                "movdqu xmm8, [{sentinels_xmm} + 128]",
                "movdqu xmm9, [{sentinels_xmm} + 144]",
                "movdqu xmm10, [{sentinels_xmm} + 160]",
                "movdqu xmm11, [{sentinels_xmm} + 176]",
                "movdqu xmm12, [{sentinels_xmm} + 192]",
                "movdqu xmm13, [{sentinels_xmm} + 208]",
                "movdqu xmm14, [{sentinels_xmm} + 224]",
                "movdqu xmm15, [{sentinels_xmm} + 240]",

                // 2. Call save routine
                "mov rdi, r12",
                "call {save_fn}",

                // 3. Zero registers
                "pxor xmm0, xmm0",
                "pxor xmm1, xmm1",
                "pxor xmm2, xmm2",
                "pxor xmm3, xmm3",
                "pxor xmm4, xmm4",
                "pxor xmm5, xmm5",
                "pxor xmm6, xmm6",
                "pxor xmm7, xmm7",
                "pxor xmm8, xmm8",
                "pxor xmm9, xmm9",
                "pxor xmm10, xmm10",
                "pxor xmm11, xmm11",
                "pxor xmm12, xmm12",
                "pxor xmm13, xmm13",
                "pxor xmm14, xmm14",
                "pxor xmm15, xmm15",

                // 4. Call restore routine
                "mov rdi, r12",
                "call {restore_fn}",

                // 5. Save registers to buffer
                "movdqu [r13], xmm0",
                "movdqu [r13 + 16], xmm1",
                "movdqu [r13 + 32], xmm2",
                "movdqu [r13 + 48], xmm3",
                "movdqu [r13 + 64], xmm4",
                "movdqu [r13 + 80], xmm5",
                "movdqu [r13 + 96], xmm6",
                "movdqu [r13 + 112], xmm7",
                "movdqu [r13 + 128], xmm8",
                "movdqu [r13 + 144], xmm9",
                "movdqu [r13 + 160], xmm10",
                "movdqu [r13 + 176], xmm11",
                "movdqu [r13 + 192], xmm12",
                "movdqu [r13 + 208], xmm13",
                "movdqu [r13 + 224], xmm14",
                "movdqu [r13 + 240], xmm15",

                sentinels_xmm = in(reg) &sentinels_xmm,
                in("r12") pstate_ptr,
                in("r13") restored_regs_ptr,
                save_fn = sym crate::save_extended_pstate,
                restore_fn = sym crate::restore_extended_pstate,
                clobber_abi("C"),
                out("rdi") _,
                out("xmm0") _, out("xmm1") _, out("xmm2") _, out("xmm3") _,
                out("xmm4") _, out("xmm5") _, out("xmm6") _, out("xmm7") _,
                out("xmm8") _, out("xmm9") _, out("xmm10") _, out("xmm11") _,
                out("xmm12") _, out("xmm13") _, out("xmm14") _, out("xmm15") _,
            );
        }

        // Assertions
        for i in 0..NUM_XMM_REGS {
            assert_eq!(restored_regs[i], sentinels_xmm[i], "restored_regs[{}] mismatch", i);
        }

        let saved_xsave = state.get_x64_xsave_area();
        for i in 0..NUM_XMM_REGS {
            let offset = 160 + i * XMM_REG_SIZE;
            let val =
                u128::from_le_bytes(saved_xsave[offset..offset + XMM_REG_SIZE].try_into().unwrap());
            assert_eq!(val, sentinels_xmm[i], "saved_xsave.xmm[{}] mismatch", i);
        }
    }
}
