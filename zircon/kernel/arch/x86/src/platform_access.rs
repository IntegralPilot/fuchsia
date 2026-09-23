// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// Lightweight trait to wrap MSR (x86 Model Specific Register) accesses.
///
/// MSR access methods allow a test to pass a fake or mock accessor to intercept `read_msr` and
/// `write_msr`.
pub trait MsrAccess {
    /// Reads a 64-bit Model Specific Register (MSR).
    fn read_msr(&self, msr_index: u32) -> u64;

    /// Writes a 64-bit value to a Model Specific Register (MSR).
    fn write_msr(&mut self, msr_index: u32, value: u64);
}

/// Real hardware MSR accessor using native CPU instructions (`rdmsr` / `wrmsr`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RealMsrAccess;

impl MsrAccess for RealMsrAccess {
    fn read_msr(&self, msr_index: u32) -> u64 {
        // SAFETY: Caller uses RealMsrAccess in kernel mode on real hardware.
        unsafe { super::x86::read_msr(msr_index) }
    }

    fn write_msr(&mut self, msr_index: u32, value: u64) {
        // SAFETY: Caller uses RealMsrAccess in kernel mode on real hardware.
        unsafe { super::x86::write_msr(msr_index, value) }
    }
}
