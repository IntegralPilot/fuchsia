// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::BTreeMap;

use crate::LayoutOver;
use crate::x86::{Cpuid, CpuidRawResult, CpuidValue, EAX, EBX, ECX, EDX};

/// A mock CPUID reader that can be populated with arbitrary leaf data.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FakeCpuid {
    /// Values by (leaf, subleaf) pairs.
    values: BTreeMap<(u32, u32), CpuidRawResult>,
}

impl FakeCpuid {
    pub const fn new() -> Self {
        Self { values: BTreeMap::new() }
    }

    /// Populates a single value for a given [`CpuidValue`].
    pub fn set<const LEAF: u32, const SUBLEAF: u32, const REG: u8, Layout: LayoutOver<u32>>(
        &mut self,
        _marker: CpuidValue<LEAF, SUBLEAF, REG, Layout>,
        value: Layout,
    ) -> &mut Self {
        let entry = self.values.entry((LEAF, SUBLEAF)).or_insert(CpuidRawResult::zeroed());
        let val: u32 = value.into();
        match REG {
            EAX => entry.eax = val,
            EBX => entry.ebx = val,
            ECX => entry.ecx = val,
            EDX => entry.edx = val,
            _ => unreachable!("REG validated on Cpuid construction"),
        }
        self
    }
}

impl Cpuid for FakeCpuid {
    fn read_raw(&self, leaf: u32, subleaf: u32) -> CpuidRawResult {
        self.values.get(&(leaf, subleaf)).copied().unwrap_or_else(CpuidRawResult::zeroed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_BASIC_LEAF: CpuidValue<0x0000_0000, 0x0, EAX, u32> = CpuidValue::new();
    const VENDOR_EBX: CpuidValue<0x0000_0000, 0x0, EBX, u32> = CpuidValue::new();
    const FEATURE_LEAF_7_EBX: CpuidValue<0x0000_0007, 0x0, EBX, u32> = CpuidValue::new();

    #[test]
    fn unpopulated_returns_zeros() {
        let cpuid = FakeCpuid::new();
        assert_eq!(cpuid.read_raw(0, 0), CpuidRawResult::zeroed());
        assert_eq!(cpuid.read(MAX_BASIC_LEAF), 0);
    }

    #[test]
    fn cpuid_populate_and_read() {
        let mut cpuid = FakeCpuid::new();
        cpuid.set(MAX_BASIC_LEAF, 7).set(VENDOR_EBX, 0x1111);
        assert_eq!(cpuid.read(MAX_BASIC_LEAF), 7);
        assert_eq!(cpuid.read(VENDOR_EBX), 0x1111);

        cpuid.set(FEATURE_LEAF_7_EBX, 0x4444);
        assert_eq!(cpuid.read(FEATURE_LEAF_7_EBX), 0x4444);
    }
}
