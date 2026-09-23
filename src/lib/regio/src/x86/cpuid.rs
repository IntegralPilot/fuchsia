// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;

use super::{EAX, EBX, ECX, EDX};
use crate::LayoutOver;

/// The raw output of a CPUID instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuidRawResult {
    /// The output EAX register.
    pub eax: u32,
    /// The output EBX register.
    pub ebx: u32,
    /// The output ECX register.
    pub ecx: u32,
    /// The output EDX register.
    pub edx: u32,
}

impl CpuidRawResult {
    pub const fn zeroed() -> Self {
        Self { eax: 0, ebx: 0, ecx: 0, edx: 0 }
    }
}

/// A marker for the CPUID layout associated with a particular leaf, subleaf,
/// and output register.
///
/// The output register, `REG`, must be one of [`EAX`], [`EBX`], [`ECX`],
/// or [`EDX`].
#[derive(Clone, Copy, Debug)]
pub struct CpuidValue<const LEAF: u32, const SUBLEAF: u32, const REG: u8, Layout: LayoutOver<u32>>(
    PhantomData<Layout>,
);

impl<const LEAF: u32, const SUBLEAF: u32, const REG: u8, Layout: LayoutOver<u32>>
    CpuidValue<LEAF, SUBLEAF, REG, Layout>
{
    pub const fn new() -> Self {
        assert!(REG <= EDX);
        Self(PhantomData {})
    }

    /// The associated leaf.
    ///
    /// Despite being a part of the type itself, it is convenient to have
    /// this accessible as an accessor, given that it is expected that one
    /// would be primarily dealing in const `Cpuid` objects.
    pub const fn leaf(&self) -> u32 {
        LEAF
    }

    /// The associated subleaf.
    ///
    /// Despite being a part of the type itself, it is convenient to have
    /// this accessible as an accessor, given that it is expected that one
    /// would be primarily dealing in const `Cpuid` objects.
    pub const fn subleaf(&self) -> u32 {
        SUBLEAF
    }
}

/// An abstracted means of reading CPUID values.
pub trait Cpuid {
    /// Returns the CPUID values for a given leaf and subleaf.
    fn read_raw(&self, leaf: u32, subleaf: u32) -> CpuidRawResult;

    /// Whether a CPUID leaf is supported, as associated with the provided
    /// [`Cpuid`] marker.
    fn supports<const LEAF: u32, const SUBLEAF: u32, const REG: u8, Layout: LayoutOver<u32>>(
        &self,
        _cpuid: CpuidValue<LEAF, SUBLEAF, REG, Layout>,
    ) -> bool {
        const MAX_BASE_LEAF: CpuidValue<0x0000_0000, 0x0, EAX, u32> = CpuidValue::new();
        const MAX_HYPERVISOR_LEAF: CpuidValue<0x4000_0000, 0x0, EAX, u32> = CpuidValue::new();
        const MAX_EXTENDED_LEAF: CpuidValue<0x8000_0000, 0x0, EAX, u32> = CpuidValue::new();

        const AMD_EXTENDED_FEATURES_C: CpuidValue<0x8000_0001, 0x0, ECX, u32> = CpuidValue::new();
        const AMD_EXTENDED_FEATURES_C_TOPOLOGY_EXTENSIONS_BIT: u32 = 22;

        const AMD_CACHE_TOPOLOGY_LEAF: u32 = 0x8000_001d;
        const AMD_PROCESSOR_TOPOLOGY_LEAF: u32 = 0x8000_001e;

        if LEAF >= MAX_EXTENDED_LEAF.leaf() {
            if LEAF > self.read(MAX_EXTENDED_LEAF) {
                return false;
            }

            // If topology extensions are not advertised, these leaves are reserved.
            if LEAF == AMD_CACHE_TOPOLOGY_LEAF || LEAF == AMD_PROCESSOR_TOPOLOGY_LEAF {
                return (self.read(AMD_EXTENDED_FEATURES_C)
                    & (1 << AMD_EXTENDED_FEATURES_C_TOPOLOGY_EXTENSIONS_BIT))
                    != 0;
            }

            return true;
        }

        if LEAF >= MAX_HYPERVISOR_LEAF.leaf() {
            return LEAF <= self.read(MAX_HYPERVISOR_LEAF);
        }

        LEAF <= self.read(MAX_BASE_LEAF)
    }

    /// Reads the particular CPUID value.
    fn read<const LEAF: u32, const SUBLEAF: u32, const REG: u8, Layout: LayoutOver<u32>>(
        &self,
        _value: CpuidValue<LEAF, SUBLEAF, REG, Layout>,
    ) -> Layout {
        let raw = self.read_raw(LEAF, SUBLEAF);
        match REG {
            EAX => raw.eax,
            EBX => raw.ebx,
            ECX => raw.ecx,
            EDX => raw.edx,
            _ => unreachable!("REG validated on Cpuid construction"),
        }
        .into()
    }

    /// Reads the particular CPUID value if supported, returning None
    /// otherwise.
    fn try_read<const LEAF: u32, const SUBLEAF: u32, const REG: u8, Layout: LayoutOver<u32>>(
        &self,
        value: CpuidValue<LEAF, SUBLEAF, REG, Layout>,
    ) -> Option<Layout> {
        self.supports(value).then(|| self.read(value))
    }
}

impl<C: Cpuid> Cpuid for &C {
    fn read_raw(&self, leaf: u32, subleaf: u32) -> CpuidRawResult {
        (*self).read_raw(leaf, subleaf)
    }
}

/// A [`CpuidReader`] that issues a CPUID instruction on each read.
#[derive(Clone, Copy, Debug)]
pub struct DirectCpuid;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86_only {
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64::{__cpuid_count, CpuidResult as ArchCpuidResult};

    #[cfg(target_arch = "x86")]
    use core::arch::x86::{__cpuid_count, CpuidResult as ArchCpuidResult};

    use super::*;

    impl Cpuid for DirectCpuid {
        #[inline]
        fn read_raw(&self, leaf: u32, subleaf: u32) -> CpuidRawResult {
            let ArchCpuidResult { eax, ebx, ecx, edx } = __cpuid_count(leaf, subleaf);
            CpuidRawResult { eax, ebx, ecx, edx }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::x86::FakeCpuid;

    const MAX_BASIC_LEAF: CpuidValue<0x0000_0000, 0x0, EAX, u32> = CpuidValue::new();
    const FEATURE_LEAF_1_ECX: CpuidValue<0x0000_0001, 0x0, ECX, u32> = CpuidValue::new();
    const FEATURE_LEAF_7_EBX: CpuidValue<0x0000_0007, 0x0, EBX, u32> = CpuidValue::new();

    const MAX_HYPERVISOR_LEAF: CpuidValue<0x4000_0000, 0x0, EAX, u32> = CpuidValue::new();
    const HYPERVISOR_LEAF_1_EAX: CpuidValue<0x4000_0001, 0x0, EAX, u32> = CpuidValue::new();
    const HYPERVISOR_LEAF_2_EAX: CpuidValue<0x4000_0002, 0x0, EAX, u32> = CpuidValue::new();

    const MAX_EXTENDED_LEAF: CpuidValue<0x8000_0000, 0x0, EAX, u32> = CpuidValue::new();
    const AMD_EXTENDED_FEATURES_C: CpuidValue<0x8000_0001, 0x0, ECX, u32> = CpuidValue::new();
    const AMD_CACHE_TOPOLOGY_A: CpuidValue<0x8000_001d, 0x0, EAX, u32> = CpuidValue::new();

    // Little more than a simple compilation test of the intended usage pattern.
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    #[test]
    fn direct_cpuid() {
        const CPUID_MAX_LEAF: CpuidValue<0x0, 0x0, EAX, u32> = CpuidValue::new();
        println!("Maximum CPUID leaf number: {:#x}", DirectCpuid {}.read(CPUID_MAX_LEAF));
    }

    #[test]
    fn cpuid_supports_basic_leaves() {
        let mut cpuid = FakeCpuid::new();
        cpuid.set(MAX_BASIC_LEAF, 1);
        assert!(cpuid.supports(FEATURE_LEAF_1_ECX));
        assert!(!cpuid.supports(FEATURE_LEAF_7_EBX));

        assert_eq!(cpuid.try_read(FEATURE_LEAF_1_ECX), Some(0));
        assert_eq!(cpuid.try_read(FEATURE_LEAF_7_EBX), None);
    }

    #[test]
    fn cpuid_supports_hypervisor_leaves() {
        let mut cpuid = FakeCpuid::new();
        cpuid.set(MAX_HYPERVISOR_LEAF, 0x4000_0001);
        assert!(cpuid.supports(HYPERVISOR_LEAF_1_EAX));

        assert!(!cpuid.supports(HYPERVISOR_LEAF_2_EAX));
    }

    #[test]
    fn cpuid_supports_extended_and_topology_leaves() {
        let mut cpuid = FakeCpuid::new();
        cpuid.set(MAX_EXTENDED_LEAF, 0x8000_001e);
        // Topology extensions not yet set in 0x8000_0001 ECX.
        assert!(!cpuid.supports(AMD_CACHE_TOPOLOGY_A));

        // Set topology extensions bit (bit 22).
        cpuid.set(AMD_EXTENDED_FEATURES_C, 1 << 22);
        assert!(cpuid.supports(AMD_CACHE_TOPOLOGY_A));
    }
}
