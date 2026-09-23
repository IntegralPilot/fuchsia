// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![no_std]

#[cfg(any(test, fuzz))]
extern crate std;

#[cfg(fuzz)]
pub mod fuzzers;
#[cfg(any(ktest, test, fuzz))]
pub mod test_data;
#[cfg(any(test, fuzz))]
pub mod test_util;
#[cfg(test)]
pub mod tests;

#[macro_use]
mod binary_reader;
mod acpi_lite;
mod apic;
mod debug_port;
mod numa;
pub mod structures;

pub use acpi_lite::{
    AcpiParser, AcpiParserInterface, AcpiTable, PhysMemReader, acpi_checksum, acpi_checksum_valid,
    get_table_by_signature, get_table_by_type, validate_rsdt, validate_xsdt,
};
pub use apic::{
    enumerate_io_apic_isa_overrides, enumerate_io_apics, enumerate_processor_local_apics,
};
pub use binary_reader::BinaryReader;
pub use debug_port::{
    AcpiDebugPortDescriptor, AcpiDebugPortType, get_debug_port, parse_acpi_dbg2_table,
};
pub use numa::{
    AcpiNumaDomain, AcpiNumaRegion, K_ACPI_MAX_NUMA_REGIONS, enumerate_cpu_numa_pairs,
    enumerate_cpu_numa_pairs_from_srat,
};
