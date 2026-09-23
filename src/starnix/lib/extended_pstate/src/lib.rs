// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(target_arch = "x86_64")]
pub use x86_64::XSAVE_AREA_SIZE as X64_XSAVE_AREA_SIZE;

#[cfg(target_arch = "x86_64")]
pub use x86_64::SUPPORTED_XSAVE_FEATURES as X64_SUPPORTED_XSAVE_FEATURES;

#[cfg(target_arch = "aarch64")]
mod aarch64;

#[cfg(target_arch = "riscv64")]
pub mod riscv64;

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct ExtendedPstateState {
    #[cfg(target_arch = "x86_64")]
    state: x86_64::State,

    #[cfg(target_arch = "aarch64")]
    state: aarch64::State,

    #[cfg(target_arch = "riscv64")]
    state: riscv64::State,
}

#[cfg(target_arch = "aarch64")]
/// A version of [`ExtendedPstateState`] that only stores the processor state
/// accessible from AArch32 (e.g., registers Q0-Q15).
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct ExtendedAarch32PstateState {
    state: aarch64::Aarch32State,
}

#[cfg(target_arch = "aarch64")]
impl ExtendedAarch32PstateState {
    pub fn reset(&mut self) {
        self.state.reset()
    }

    pub fn get_arm32_qregs(&self) -> &[u128; 16] {
        &self.state.q
    }

    pub fn get_arm32_fpsr(&self) -> u32 {
        self.state.fpsr
    }

    pub fn get_arm32_fpcr(&self) -> u32 {
        self.state.fpcr
    }
}

impl ExtendedPstateState {
    #[cfg(target_arch = "x86_64")]
    pub fn with_strategy(strategy: x86_64::Strategy) -> Self {
        Self { state: x86_64::State::with_strategy(strategy) }
    }

    pub fn reset(&mut self) {
        self.state.reset()
    }

    #[cfg(target_arch = "aarch64")]
    pub fn get_arm64_qregs(&self) -> &[u128; 32] {
        &self.state.q
    }

    #[cfg(target_arch = "aarch64")]
    pub fn get_arm64_fpsr(&self) -> u32 {
        self.state.fpsr
    }

    #[cfg(target_arch = "aarch64")]
    pub fn get_arm64_fpcr(&self) -> u32 {
        self.state.fpcr
    }

    #[cfg(target_arch = "aarch64")]
    pub fn set_arm64_state(&mut self, qregs: &[u128; 32], fpsr: u32, fpcr: u32) {
        self.state.q = *qregs;
        self.state.fpsr = fpsr;
        self.state.fpcr = fpcr;
    }

    #[cfg(target_arch = "riscv64")]
    pub fn get_riscv64_state(&self) -> &riscv64::State {
        &self.state
    }

    #[cfg(target_arch = "riscv64")]
    pub fn get_riscv64_state_mut(&mut self) -> &mut riscv64::State {
        &mut self.state
    }

    #[cfg(target_arch = "x86_64")]
    pub fn get_x64_xsave_area(&self) -> [u8; X64_XSAVE_AREA_SIZE] {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            std::mem::transmute(self.state.buffer)
        }
    }

    #[cfg(target_arch = "x86_64")]
    pub fn set_x64_xsave_area(&mut self, xsave_area: [u8; X64_XSAVE_AREA_SIZE]) {
        self.state.set_xsave_area(xsave_area);
    }
}

/// Stores a pointer to the currently active extended pstate storage.
/// The caller to the C entry points is responsible for ensuring that the active union
/// member corresponds to the entry points being called.
#[repr(C)]
pub union ExtendedPstatePointer {
    pub extended_pstate: *mut ExtendedPstateState,
    #[cfg(target_arch = "aarch64")]
    pub extended_aarch32_pstate: *mut ExtendedAarch32PstateState,
}

// Provided by assembly targets.
unsafe extern "C" {
    pub fn save_extended_pstate(state_addr: usize);
    pub fn restore_extended_pstate(state_addr: usize);

    #[cfg(target_arch = "aarch64")]
    pub fn save_extended_aarch32_pstate(state_addr: usize);
    #[cfg(target_arch = "aarch64")]
    pub fn restore_extended_aarch32_pstate(state_addr: usize);
}
