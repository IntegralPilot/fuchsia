// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use regio::x86::{Cpuid, CpuidValue, EAX};

use super::cpuid::{
    AMD_CACHE_TOPOLOGY_A, AMD_L1_DATA_CACHE_INFO, AMD_L1_INSTRUCTION_CACHE_INFO, AMD_L2_CACHE_INFO,
    AMD_L3_CACHE_INFO, CacheTopologyA, CacheTopologyB, CacheTopologyC, CacheType,
    INTEL_CACHE_TOPOLOGY_A,
};

/// Represents a single cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuCacheLevelInfo {
    pub level: usize,
    pub cache_type: CacheType,

    /// The size, in KiB, of the cache available to each processor. In the case of
    /// the last-level cache, however, this field might report the aggregate size
    /// of all such caches on the package.
    pub size_kb: usize,

    /// The number of sets in the cache available to each processor. In the case
    /// of the last-level cache, however, this field might report the aggregate
    /// number of sets across all such caches in the package.
    /// Indeterminate if zero.
    pub number_of_sets: usize,

    /// Indeterminate if zero.
    pub ways_of_associativity: usize,

    /// Indeterminate if None.
    pub fully_associative: Option<bool>,

    /// The number of bits to shift an APIC ID to get the associated "share ID":
    /// processors with coinciding share IDs share this cache. If None, then it
    /// is indeterminate what the cache's shift is.
    pub share_id_shift: Option<usize>,
}

impl CpuCacheLevelInfo {
    const NULL: Self = Self {
        level: 0,
        cache_type: CacheType::Null,
        size_kb: 0,
        number_of_sets: 0,
        ways_of_associativity: 0,
        fully_associative: None,
        share_id_shift: None,
    };
}

impl Default for CpuCacheLevelInfo {
    fn default() -> Self {
        Self::NULL
    }
}

const fn ceil_log2(n: usize) -> usize {
    n.next_power_of_two().trailing_zeros() as usize
}

/// Gives information on the set of caches in a package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuCacheInfo {
    caches: [CpuCacheLevelInfo; Self::MAX_NUM_CACHES],
    size: usize,
}

impl CpuCacheInfo {
    /// A split L1 and unified L2, L3, L4 caches makes five.
    pub const MAX_NUM_CACHES: usize = 5;

    pub fn from_cpuid(cpuid: impl Cpuid) -> Option<Self> {
        // We first try the Intel v2 leaves - and then the AMD v2 leaves.
        // Hypervisors on AMD hosts might lay CPUID values in the Intel style - and
        // there is no harm in doing this in general as AMD hardware will tend to
        // reserve these Intel leaves as zero.
        if let Some(info) = Self::from_v2_topology(&cpuid, INTEL_CACHE_TOPOLOGY_A) {
            return Some(info);
        }
        if let Some(info) = Self::from_v2_topology(&cpuid, AMD_CACHE_TOPOLOGY_A) {
            return Some(info);
        }

        Self::from_v1_amd_topology(&cpuid)
    }

    pub fn as_slice(&self) -> &[CpuCacheLevelInfo] {
        &self.caches[..self.size]
    }

    /// Returns information on the last-level cache.
    pub fn last_level(&self) -> &CpuCacheLevelInfo {
        // Construction guarantees size >= 3.
        self.as_slice().last().unwrap()
    }

    fn from_v2_topology<const LEAF: u32>(
        cpuid: &impl Cpuid,
        topology_0th: CpuidValue<LEAF, 0, EAX, CacheTopologyA>,
    ) -> Option<Self> {
        if !cpuid.supports(topology_0th) {
            return None;
        }

        let mut caches = [CpuCacheLevelInfo::NULL; Self::MAX_NUM_CACHES];
        let mut size = 0;
        for subleaf in 0..Self::MAX_NUM_CACHES {
            let raw = cpuid.read_raw(LEAF, subleaf as u32);
            let eax = CacheTopologyA::from(raw.eax);
            if eax.cache_type() == CacheType::Null {
                break;
            }
            let ebx = CacheTopologyB::from(raw.ebx);
            let ecx = CacheTopologyC::from(raw.ecx);

            let size_bytes = (ebx.ways() as usize + 1)
                * (ebx.physical_line_partitions() as usize + 1)
                * (ebx.system_coherency_line_size() as usize + 1)
                * (ecx.sets() as usize + 1);

            caches[size] = CpuCacheLevelInfo {
                level: eax.cache_level() as usize,
                cache_type: eax.cache_type(),
                size_kb: size_bytes / 1024,
                number_of_sets: ecx.sets() as usize + 1,
                ways_of_associativity: ebx.ways() as usize + 1,
                fully_associative: Some(eax.fully_associative()),
                share_id_shift: Some(ceil_log2(eax.max_sharing_logical_processors() as usize + 1)),
            };
            size += 1;
        }

        // We expect at least split L1 caches and an L2 cache.
        if size >= 3 { Some(Self { caches, size }) } else { None }
    }

    fn from_v1_amd_topology(cpuid: &impl Cpuid) -> Option<Self> {
        // The extended leaves explicitly enumerate information about L1d, L1i, L2,
        // and L3, which was the original means of figuring out cache topology on
        // AMD.
        if !cpuid.supports(AMD_L3_CACHE_INFO) {
            return None;
        }

        let l1d = cpuid.read(AMD_L1_DATA_CACHE_INFO);
        let l1i = cpuid.read(AMD_L1_INSTRUCTION_CACHE_INFO);
        let l2 = cpuid.read(AMD_L2_CACHE_INFO);
        let l3 = cpuid.read(AMD_L3_CACHE_INFO);

        let mut caches = [CpuCacheLevelInfo::NULL; Self::MAX_NUM_CACHES];
        caches[0] = CpuCacheLevelInfo {
            level: 1,
            cache_type: CacheType::Data,
            size_kb: l1d.size_kb() as usize,
            number_of_sets: 0,
            ways_of_associativity: l1d.ways_of_associativity(),
            fully_associative: l1d.fully_associative(),
            share_id_shift: None,
        };
        caches[1] = CpuCacheLevelInfo {
            level: 1,
            cache_type: CacheType::Instruction,
            size_kb: l1i.size_kb() as usize,
            number_of_sets: 0,
            ways_of_associativity: l1i.ways_of_associativity(),
            fully_associative: l1i.fully_associative(),
            share_id_shift: None,
        };
        caches[2] = CpuCacheLevelInfo {
            level: 2,
            cache_type: CacheType::Unified,
            size_kb: l2.size_kb() as usize,
            number_of_sets: 0,
            ways_of_associativity: l2.ways_of_associativity(),
            fully_associative: l2.fully_associative(),
            share_id_shift: None,
        };
        let mut size = 3;

        if l3.size() != 0 {
            // [amd/vol3]: E.4.5  Function 8000_0006h—L2 Cache and TLB and L3 Cache Information.
            //
            // `l3.size()` actually provides bounds for the total size of L3
            // cache across the package, in terms of 512 KiB blocks:
            // l3.size() * 512 ≤ total size KiB < (l3.size() + 1) * 512
            // In practice, the total size is a multiple of 512 and this
            // reports the actual total size.
            caches[3] = CpuCacheLevelInfo {
                level: 3,
                cache_type: CacheType::Unified,
                size_kb: 512 * l3.size() as usize,
                number_of_sets: 0,
                ways_of_associativity: l3.ways_of_associativity(),
                fully_associative: l3.fully_associative(),
                share_id_shift: None,
            };
            size = 4;
        }

        Some(Self { caches, size })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::x86::cpuid::{
        AmdL1CacheInformation, AmdL2CacheInformation, AmdL2L3Associativity, AmdL3CacheInformation,
        MAX_LEAF,
    };
    use regio::testing::x86::FakeCpuid;
    use regio::x86::{EBX, ECX};

    #[test]
    fn intel_v2_topology() {
        let mut cpuid = FakeCpuid::new();
        cpuid.set(MAX_LEAF, 4);

        // Subleaf 0: L1 Data cache.
        // ways = 7 (8-way), physical_line_partitions = 0 (1), system_coherency_line_size = 63 (64 bytes), sets = 63 (64 sets) -> 32 KiB
        let l1d_a = *CacheTopologyA::new()
            .set_cache_level(1)
            .set_cache_type(CacheType::Data)
            .set_max_sharing_logical_processors(1) // 2 processors sharing -> share_id_shift = 1
            .set_fully_associative(false);
        let l1d_b = *CacheTopologyB::new()
            .set_ways(7)
            .set_physical_line_partitions(0)
            .set_system_coherency_line_size(63);
        let l1d_c = *CacheTopologyC::new().set_sets(63);

        // Subleaf 1: L1 Instruction cache.
        let l1i_a = *CacheTopologyA::new()
            .set_cache_level(1)
            .set_cache_type(CacheType::Instruction)
            .set_max_sharing_logical_processors(1)
            .set_fully_associative(false);
        let l1i_b = *CacheTopologyB::new()
            .set_ways(7)
            .set_physical_line_partitions(0)
            .set_system_coherency_line_size(63);
        let l1i_c = *CacheTopologyC::new().set_sets(63);

        // Subleaf 2: L2 Unified cache.
        // ways = 7 (8-way), physical_line_partitions = 0 (1), system_coherency_line_size = 63 (64 bytes), sets = 511 (512 sets) -> 256 KiB
        let l2_a = *CacheTopologyA::new()
            .set_cache_level(2)
            .set_cache_type(CacheType::Unified)
            .set_max_sharing_logical_processors(1)
            .set_fully_associative(false);
        let l2_b = *CacheTopologyB::new()
            .set_ways(7)
            .set_physical_line_partitions(0)
            .set_system_coherency_line_size(63);
        let l2_c = *CacheTopologyC::new().set_sets(511);

        // Subleaf 3: Null cache (ends enumeration).
        let l3_a = *CacheTopologyA::new().set_cache_type(CacheType::Null);

        cpuid
            .set(CpuidValue::<4, 0, EAX, CacheTopologyA>::new(), l1d_a)
            .set(CpuidValue::<4, 0, EBX, CacheTopologyB>::new(), l1d_b)
            .set(CpuidValue::<4, 0, ECX, CacheTopologyC>::new(), l1d_c)
            .set(CpuidValue::<4, 1, EAX, CacheTopologyA>::new(), l1i_a)
            .set(CpuidValue::<4, 1, EBX, CacheTopologyB>::new(), l1i_b)
            .set(CpuidValue::<4, 1, ECX, CacheTopologyC>::new(), l1i_c)
            .set(CpuidValue::<4, 2, EAX, CacheTopologyA>::new(), l2_a)
            .set(CpuidValue::<4, 2, EBX, CacheTopologyB>::new(), l2_b)
            .set(CpuidValue::<4, 2, ECX, CacheTopologyC>::new(), l2_c)
            .set(CpuidValue::<4, 3, EAX, CacheTopologyA>::new(), l3_a);

        let info = CpuCacheInfo::from_cpuid(&cpuid).unwrap();
        assert_eq!(
            info.as_slice(),
            &[
                CpuCacheLevelInfo {
                    level: 1,
                    cache_type: CacheType::Data,
                    size_kb: 32,
                    number_of_sets: 64,
                    ways_of_associativity: 8,
                    fully_associative: Some(false),
                    share_id_shift: Some(1),
                },
                CpuCacheLevelInfo {
                    level: 1,
                    cache_type: CacheType::Instruction,
                    size_kb: 32,
                    number_of_sets: 64,
                    ways_of_associativity: 8,
                    fully_associative: Some(false),
                    share_id_shift: Some(1),
                },
                CpuCacheLevelInfo {
                    level: 2,
                    cache_type: CacheType::Unified,
                    size_kb: 256,
                    number_of_sets: 512,
                    ways_of_associativity: 8,
                    fully_associative: Some(false),
                    share_id_shift: Some(1),
                },
            ]
        );
        assert_eq!(info.last_level(), &info.as_slice()[2]);
    }

    #[test]
    fn amd_v1_topology() {
        let mut cpuid = FakeCpuid::new();
        // Extended max leaf >= 0x8000_0006.
        cpuid.set(CpuidValue::<0x8000_0000, 0, EAX, u32>::new(), 0x8000_0006);

        // L1 Data: 32 KiB, 8-way.
        let l1d = *AmdL1CacheInformation::new().set_size_kb(32).set_assoc(8);
        // L1 Inst: 64 KiB, 2-way.
        let l1i = *AmdL1CacheInformation::new().set_size_kb(64).set_assoc(2);
        // L2: 512 KiB, 8-way.
        let l2 =
            *AmdL2CacheInformation::new().set_size_kb(512).set_assoc(AmdL2L3Associativity::Ways8);
        // L3: 8 * 512 = 4096 KiB, 16-way.
        let l3 = *AmdL3CacheInformation::new().set_size(8).set_assoc(AmdL2L3Associativity::Ways16);

        cpuid
            .set(AMD_L1_DATA_CACHE_INFO, l1d)
            .set(AMD_L1_INSTRUCTION_CACHE_INFO, l1i)
            .set(AMD_L2_CACHE_INFO, l2)
            .set(AMD_L3_CACHE_INFO, l3);

        let info = CpuCacheInfo::from_cpuid(&cpuid).unwrap();
        assert_eq!(
            info.as_slice(),
            &[
                CpuCacheLevelInfo {
                    level: 1,
                    cache_type: CacheType::Data,
                    size_kb: 32,
                    number_of_sets: 0,
                    ways_of_associativity: 8,
                    fully_associative: Some(false),
                    share_id_shift: None,
                },
                CpuCacheLevelInfo {
                    level: 1,
                    cache_type: CacheType::Instruction,
                    size_kb: 64,
                    number_of_sets: 0,
                    ways_of_associativity: 2,
                    fully_associative: Some(false),
                    share_id_shift: None,
                },
                CpuCacheLevelInfo {
                    level: 2,
                    cache_type: CacheType::Unified,
                    size_kb: 512,
                    number_of_sets: 0,
                    ways_of_associativity: 8,
                    fully_associative: Some(false),
                    share_id_shift: None,
                },
                CpuCacheLevelInfo {
                    level: 3,
                    cache_type: CacheType::Unified,
                    size_kb: 4096,
                    number_of_sets: 0,
                    ways_of_associativity: 16,
                    fully_associative: Some(false),
                    share_id_shift: None,
                },
            ]
        );
        assert_eq!(info.last_level(), &info.as_slice()[3]);
    }
}
