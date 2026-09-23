// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::binary_reader::{BinaryReader, DowncastFrom};
use crate::structures::{
    ACPI_SRAT_FLAG_ENABLED, ACPI_SRAT_TYPE_MEMORY_AFFINITY, ACPI_SRAT_TYPE_PROCESSOR_AFFINITY,
    ACPI_SRAT_TYPE_PROCESSOR_X2APIC_AFFINITY, AcpiSratMemoryAffinityEntry,
    AcpiSratProcessorAffinityEntry, AcpiSratProcessorX2ApicAffinityEntry, AcpiSratTable,
    AcpiSubTableHeader,
};
use crate::{AcpiParserInterface, get_table_by_type};
use zx_status::Status;

pub const K_ACPI_MAX_NUMA_REGIONS: usize = 5;

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
// A region of memory associated with a NUMA domain.
pub struct AcpiNumaRegion {
    pub base_address: u64,
    pub length: u64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
// A NUMA domain.
pub struct AcpiNumaDomain {
    pub domain: u32,
    pub memory: [AcpiNumaRegion; K_ACPI_MAX_NUMA_REGIONS],
    pub memory_count: u8,
}

impl_downcast_from!(AcpiSubTableHeader =>
    AcpiSratMemoryAffinityEntry,
    AcpiSratProcessorAffinityEntry,
    AcpiSratProcessorX2ApicAffinityEntry,
);

// Calls the given callback on all pairs of CPU APIC ID and NumaRegion.
pub fn enumerate_cpu_numa_pairs_from_srat<F>(
    srat: &AcpiSratTable,
    mut callback: F,
) -> Result<(), Status>
where
    F: FnMut(&AcpiNumaDomain, u32),
{
    const K_MAX_NUMA_DOMAINS: usize = 10;
    let mut domains = [AcpiNumaDomain::default(); K_MAX_NUMA_DOMAINS];
    for (i, domain) in domains.iter_mut().enumerate() {
        domain.domain = i as u32;
    }

    // First find all NUMA domains.
    let mut reader = BinaryReader::from_payload_of_struct(srat);
    while !reader.is_empty() {
        let sub_header = reader.read::<AcpiSubTableHeader>().ok_or(Status::INTERNAL)?;
        if sub_header.r#type != ACPI_SRAT_TYPE_MEMORY_AFFINITY {
            continue;
        }
        // SAFETY: We verified that `sub_header.r#type` is `ACPI_SRAT_TYPE_MEMORY_AFFINITY`.
        let mem = unsafe { AcpiSratMemoryAffinityEntry::downcast_from(sub_header) }
            .ok_or(Status::INTERNAL)?;

        let flags = mem.flags;
        if (flags & ACPI_SRAT_FLAG_ENABLED) == 0 {
            continue;
        }

        let proximity_domain = mem.proximity_domain as usize;
        if proximity_domain >= K_MAX_NUMA_DOMAINS {
            return Err(Status::NOT_SUPPORTED);
        }

        let domain = &mut domains[proximity_domain];
        if domain.memory_count as usize >= K_ACPI_MAX_NUMA_REGIONS {
            return Err(Status::NOT_SUPPORTED);
        }

        let base_low = mem.base_address_low as u64;
        let base_high = mem.base_address_high as u64;
        let length_low = mem.length_low as u64;
        let length_high = mem.length_high as u64;
        let base = (base_high << 32) | base_low;
        let length = (length_high << 32) | length_low;

        domain.memory[domain.memory_count as usize] = AcpiNumaRegion { base_address: base, length };
        domain.memory_count += 1;
    }

    // Then visit all CPU APIC IDs and provide the accompanying NUMA region.
    reader = BinaryReader::from_payload_of_struct(srat);
    while !reader.is_empty() {
        let sub_header = reader.read::<AcpiSubTableHeader>().ok_or(Status::INTERNAL)?;
        if sub_header.r#type == ACPI_SRAT_TYPE_PROCESSOR_AFFINITY {
            // SAFETY: We verified that `sub_header.r#type` is `ACPI_SRAT_TYPE_PROCESSOR_AFFINITY`.
            let cpu = unsafe { AcpiSratProcessorAffinityEntry::downcast_from(sub_header) }
                .ok_or(Status::INTERNAL)?;

            let flags = cpu.flags;
            if (flags & ACPI_SRAT_FLAG_ENABLED) == 0 {
                continue;
            }

            let domain = cpu.proximity_domain() as usize;
            if domain >= K_MAX_NUMA_DOMAINS {
                return Err(Status::INTERNAL);
            }

            let apic_id = cpu.apic_id as u32;
            callback(&domains[domain], apic_id);
        } else if sub_header.r#type == ACPI_SRAT_TYPE_PROCESSOR_X2APIC_AFFINITY {
            // SAFETY: We verified that `sub_header.r#type` is `ACPI_SRAT_TYPE_PROCESSOR_X2APIC_AFFINITY`.
            let cpu = unsafe { AcpiSratProcessorX2ApicAffinityEntry::downcast_from(sub_header) }
                .ok_or(Status::INTERNAL)?;

            let flags = cpu.flags;
            if (flags & ACPI_SRAT_FLAG_ENABLED) == 0 {
                continue;
            }

            let domain = cpu.proximity_domain as usize;
            if domain >= K_MAX_NUMA_DOMAINS {
                return Err(Status::INTERNAL);
            }

            let x2apic_id = cpu.x2apic_id;
            callback(&domains[domain], x2apic_id);
        }
    }

    Ok(())
}

// Calls the given callback on all pairs of CPU APIC ID and NumaRegion.
pub fn enumerate_cpu_numa_pairs<F>(
    parser: &dyn AcpiParserInterface,
    callback: F,
) -> Result<(), Status>
where
    F: FnMut(&AcpiNumaDomain, u32),
{
    let srat = get_table_by_type::<AcpiSratTable>(parser).ok_or(Status::NOT_FOUND)?;
    enumerate_cpu_numa_pairs_from_srat(srat, callback)
}
