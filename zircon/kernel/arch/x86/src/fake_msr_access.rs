// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// A single MSR register index and value pair for fake/mock access.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FakeMsr {
    pub index: u32,
    pub value: u64,
}

/// A fake implementation of [`MsrAccess`], allowing unit tests to verify MSR reads and writes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FakeMsrAccess {
    pub msrs: [FakeMsr; 4],
    pub no_writes: bool,
}

impl super::platform_access::MsrAccess for FakeMsrAccess {
    fn read_msr(&self, msr_index: u32) -> u64 {
        for msr in &self.msrs {
            if msr.index == msr_index {
                return msr.value;
            }
        }
        panic!("Attempted to read unknown MSR {:#x}.", msr_index);
    }

    fn write_msr(&mut self, msr_index: u32, value: u64) {
        debug_assert!(!self.no_writes, "write_msr called when no_writes is true");
        for msr in &mut self.msrs {
            if msr.index == msr_index {
                msr.value = value;
                return;
            }
        }
        panic!("Attempted to write unknown MSR {:#x} with value {:#x}.", msr_index, value);
    }
}
