// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use regio::x86::{Cpuid, CpuidValue, EAX};

use super::cpuid::{
    COMPUTE_UNIT_INFO, CacheType, EXTENDED_APIC_ID, EXTENDED_SIZE_INFO, FEATURE_FLAGS_D,
    INTEL_CACHE_TOPOLOGY_A, NODE_INFO, PROCESSOR_INFO, TopologyEnumerationA, TopologyEnumerationC,
    TopologyLevelType, V1_TOPOLOGY_A, V1_TOPOLOGY_C, V1_TOPOLOGY_D, V2_TOPOLOGY_A, V2_TOPOLOGY_C,
    V2_TOPOLOGY_D,
};

/// Returns the APIC ID - x2APIC if supported - associated with the logical
/// processor in turn associated with the provided [`Cpuid`].
pub fn get_apic_id(cpuid: impl Cpuid) -> u32 {
    // [intel/vol3]: 8.9.2  Hierarchical Mapping of CPUID Extended Topology Leaf.
    //
    // For extended topology enumeration, if the first level does not encode the
    // "SMT" level (a spec'ed expectation), then we assume the associated leaves
    // to be invalid.
    if cpuid.supports(V2_TOPOLOGY_A)
        && cpuid.read(V2_TOPOLOGY_C).level_type() == TopologyLevelType::Smt
    {
        return cpuid.read(V2_TOPOLOGY_D).x2apic_id();
    }
    if cpuid.supports(V1_TOPOLOGY_A)
        && cpuid.read(V1_TOPOLOGY_C).level_type() == TopologyLevelType::Smt
    {
        return cpuid.read(V1_TOPOLOGY_D).x2apic_id();
    }

    if cpuid.supports(EXTENDED_APIC_ID) {
        return cpuid.read(EXTENDED_APIC_ID).x2apic_id();
    }

    cpuid.read(PROCESSOR_INFO).initial_apic_id() as u32
}

/// [`ApicIdDecoder`] is a utility for extracting particular topological level
/// IDs from an (x2)APIC ID.
///
/// In full generality, an APIC ID might decompose as follows:
///
/// [intel/vol3]: Figure 8-5.  Generalized Seven Level Interpretation of the APIC ID.
/// -----------------------------------------------------------------------------
/// | CLUSTER ID | PACKAGE ID | DIE ID | TILE ID | MODULE ID | CORE ID | SMT ID |
/// -----------------------------------------------------------------------------
///
/// where the full ID width is 32-bit (if x2APIC) or 8-bit.
///
/// This, however, is higher fidelity than we are able to make use of. Since
/// CLUSTER ID and PACKAGE_ID are not directly enumerable from CPUID, we elide
/// the two IDs into a single PACKAGE ID, defined as the rest of the ID above
/// DIE. Moreover, the system currently has no use for enumerating tiles and
/// modules directly (which is also a practice that AMD does not do): we elide
/// the TILE and MODULE IDs into DIE ID alone. Accordingly, [`ApicIdDecoder`]
/// partitions up the APIC address space as
/// ------------------------------------------
/// | PACKAGE ID | DIE ID | CORE ID | SMT ID |
/// ------------------------------------------
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApicIdDecoder {
    smt_id_width: usize,
    // CORE ID width + SMT ID width.
    core_id_cumulative_width: usize,
    // DIE ID width + CORE ID width + SMT ID width.
    die_id_cumulative_width: usize,
}

impl ApicIdDecoder {
    const MAX_TOPOLOGY_LEVEL: usize = 5; // TopologyLevelType::Die as usize

    /// Constructs a new [`ApicIdDecoder`] by querying the given [`Cpuid`].
    pub fn from_cpuid(cpuid: impl Cpuid) -> Self {
        let mut decoder = Self::default();

        // [intel/vol3]: Example 8-21.  Support Routines for Identifying Package,
        // Core and Logical Processors from 8-bit Initial APIC ID.
        // [amd/vol3]: E.5.1  Legacy Method.
        //
        // When HTT ("Hyper-Threading Technology") is not advertised, the package
        // contains a single logical processor. This is counter-intuitive, but
        // Intel cores that do not actually have SMT available may still present
        // HTT == 1; moreover, in the case of AMD, HTT means "either that there is
        // more than one thread per core or more than one core per compute unit".
        if !cpuid.read(FEATURE_FLAGS_D).htt() {
            return decoder;
        }

        // First try the extended topology leaves, which may work with older AMD
        // models. The "V2" leaf 0x1f is preferred - if available - to the "V1"
        // leaf 0xb.
        if decoder.try_extended_topology(&cpuid, V2_TOPOLOGY_A)
            || decoder.try_extended_topology(&cpuid, V1_TOPOLOGY_A)
        {
            // The DIE level might not have been explicitly enumerated. If it does
            // not seem so, redefine the cumulative die-and-below ID width to be the
            // rounded binary order of the maximum number of addressable logical
            // processors per package, which should always coincide in general.
            if decoder.die_id_cumulative_width == decoder.core_id_cumulative_width {
                decoder.die_id_cumulative_width =
                    Self::ceil_log2(Self::max_num_logical_processors(&cpuid));
            }
            return decoder;
        }

        // Maximum per package, that is.
        let max_logical_processors = Self::max_num_logical_processors(&cpuid);
        let mut max_cores = 1;
        let mut max_dies = 1;

        // [intel/vol3]: Example 8-21.  Support Routines for Identifying
        // Package, Core and Logical Processors from 8-bit Initial APIC ID.
        if cpuid.supports(INTEL_CACHE_TOPOLOGY_A) {
            let zeroth_cache_topology = cpuid.read(INTEL_CACHE_TOPOLOGY_A);
            if zeroth_cache_topology.cache_type() != CacheType::Null {
                // The field encodes one less than the real count.
                max_cores = zeroth_cache_topology.max_cores() as usize + 1;
                decoder.finalize(max_logical_processors, max_cores, max_dies);
                return decoder;
            }
        }

        // Unfortunately, the AMD spec does not give a general way of
        // determining the maximum number of addressable cores and dies per
        // package, respectively. If leaf 0x8000'001e is supported (which
        // requires the topology extension feature to be advertised), then we
        // can give best-effort guesses of these quantities based on the actual
        // counts of dies per package and logical processors per core.
        if cpuid.supports(COMPUTE_UNIT_INFO) {
            // We translate "compute unit" and "node" here as core and die,
            // respectively.
            max_dies = cpuid.read(NODE_INFO).nodes_per_package() as usize + 1;
            let threads_per_core =
                cpuid.read(COMPUTE_UNIT_INFO).threads_per_compute_unit() as usize + 1;
            max_cores = max_logical_processors / threads_per_core;
        }
        decoder.finalize(max_logical_processors, max_cores, max_dies);
        decoder
    }

    /// Extracts the SMT ID from the provided APIC ID.
    pub const fn smt_id(&self, apic_id: u32) -> u32 {
        apic_id & Self::to_mask(self.smt_id_width)
    }

    /// Extracts the Core ID from the provided APIC ID.
    pub const fn core_id(&self, apic_id: u32) -> u32 {
        (apic_id & Self::to_mask(self.core_id_cumulative_width)) >> self.smt_id_width
    }

    /// Extracts the Die ID from the provided APIC ID.
    pub const fn die_id(&self, apic_id: u32) -> u32 {
        (apic_id & Self::to_mask(self.die_id_cumulative_width)) >> self.core_id_cumulative_width
    }

    /// Extracts the Package ID from the provided APIC ID.
    pub const fn package_id(&self, apic_id: u32) -> u32 {
        if self.die_id_cumulative_width >= 32 { 0 } else { apic_id >> self.die_id_cumulative_width }
    }

    /// Returns the bit width allocated to the SMT ID.
    pub const fn smt_id_width(&self) -> usize {
        self.smt_id_width
    }

    /// Returns the cumulative bit width allocated to Core and SMT IDs.
    pub const fn core_id_cumulative_width(&self) -> usize {
        self.core_id_cumulative_width
    }

    /// Returns the cumulative bit width allocated to Die, Core, and SMT IDs.
    pub const fn die_id_cumulative_width(&self) -> usize {
        self.die_id_cumulative_width
    }

    // [intel/vol3]: Example 8-18.  Support Routines for Identifying Package,
    // Die, Core and Logical Processors from 32-bit x2APIC ID.
    //
    // Attempts to perform Intel's extended topology enumeration routine and
    // returns whether the attempt was successful.
    fn try_extended_topology<const LEAF: u32>(
        &mut self,
        cpuid: &impl Cpuid,
        topology_0th: CpuidValue<LEAF, 0, EAX, TopologyEnumerationA>,
    ) -> bool {
        if !cpuid.supports(topology_0th) {
            return false;
        }

        for i in 0..Self::MAX_TOPOLOGY_LEVEL {
            let raw = cpuid.read_raw(LEAF, i as u32);
            let eax = TopologyEnumerationA::from(raw.eax);
            let ecx = TopologyEnumerationC::from(raw.ecx);

            // The above reference explains that SMT is expected to be the first level.
            let level_type = ecx.level_type();
            if i == 0 && level_type != TopologyLevelType::Smt {
                return false;
            }
            let shift = eax.next_level_apic_id_shift() as usize;
            match level_type {
                TopologyLevelType::Invalid => return true, // Signals the end of iteration.
                TopologyLevelType::Smt => {
                    self.smt_id_width = shift;
                    self.core_id_cumulative_width = shift;
                    self.die_id_cumulative_width = shift;
                }
                TopologyLevelType::Core => {
                    self.core_id_cumulative_width = shift;
                    self.die_id_cumulative_width = shift;
                }
                // See class documentation regarding the elision of MODULE and TILE.
                TopologyLevelType::Module | TopologyLevelType::Tile | TopologyLevelType::Die => {
                    self.die_id_cumulative_width = shift;
                }
            }
        }

        // Something went wrong; iteration should have finished in hitting on a
        // TopologyLevelType::Invalid level.
        false
    }

    fn finalize(&mut self, max_logical_processors: usize, max_cores: usize, max_dies: usize) {
        if !(max_logical_processors >= max_cores && max_cores >= max_dies && max_dies > 0) {
            return;
        }

        self.smt_id_width = Self::ceil_log2(max_logical_processors / max_cores);
        self.core_id_cumulative_width = Self::ceil_log2(max_cores / max_dies) + self.smt_id_width;
        self.die_id_cumulative_width = Self::ceil_log2(max_logical_processors);
    }

    // Returns the maximum addressable number of logical processors per package.
    // Both Intel and AMD spec ways to determine this quantity.
    fn max_num_logical_processors(cpuid: &impl Cpuid) -> usize {
        // The Intel max.
        let mut max = cpuid.read(PROCESSOR_INFO).max_logical_processors() as usize;

        // The AMD max. For AMD hardware, the quantity above gives the actual count
        // of logical processors instead of the maximum number of addressable ones.
        if cpuid.supports(EXTENDED_SIZE_INFO) {
            // [amd/vol3]: E.5.2  Extended Method.
            let size_ids = cpuid.read(EXTENDED_SIZE_INFO);
            let amd_max = if size_ids.apic_id_size() != 0 {
                1usize << size_ids.apic_id_size()
            } else {
                size_ids.nc() as usize + 1
            };
            max = max.max(amd_max);
        }
        max
    }

    const fn ceil_log2(n: usize) -> usize {
        n.next_power_of_two().trailing_zeros() as usize
    }

    const fn to_mask(width: usize) -> u32 {
        if width >= 32 { !0 } else { !(!0u32 << width) }
    }
}
