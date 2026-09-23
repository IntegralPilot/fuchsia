// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![cfg(test)]

extern crate std;

use crate::structures::{AcpiHpetTable, AcpiRsdt, AcpiSdtHeader, AcpiSignature, VariableSized};
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use crate::structures::{K_BIOS_READ_ONLY_AREA_LENGTH, K_BIOS_READ_ONLY_AREA_START};
use crate::test_util::{
    EmptyPhysMemReader, FakeAcpiParser, FakePhysMemReader, FakeRegion, NullPhysMemReader,
    fuchsia_hypervisor_phys_mem_reader, intel_nuc7i5dn_phys_mem_reader, qemu_phys_mem_reader,
};
use crate::{
    AcpiParser, AcpiParserInterface, AcpiTable, PhysMemReader, acpi_checksum, acpi_checksum_valid,
    get_table_by_signature, get_table_by_type, validate_rsdt,
};
use zx_status::Status;

#[test]
fn test_no_rsdp() {
    let reader = NullPhysMemReader;
    let result = AcpiParser::init(&reader, 0);
    assert!(result.is_err());
    assert_eq!(result.err().unwrap(), Status::NOT_FOUND);
}

#[test]
fn test_empty_tables() {
    let reader = EmptyPhysMemReader::new();
    let result = AcpiParser::init(&reader, 0);
    assert!(result.is_err());
    assert_eq!(result.err().unwrap(), Status::NOT_FOUND);
}

fn verify_table_exists(parser: &dyn AcpiParserInterface, signature: &str) {
    let sig = AcpiSignature::new(signature.as_bytes().try_into().unwrap());
    let table = get_table_by_signature(parser, sig).expect("Table does not exist");

    assert_eq!(&table.sig.0, signature.as_bytes());
    assert!(table.size() >= core::mem::size_of::<AcpiSdtHeader>());
}

#[test]
fn test_parse_qemu_tables() {
    let reader = qemu_phys_mem_reader();
    let result = AcpiParser::init(&reader, reader.rsdp()).unwrap();
    assert_eq!(result.num_tables(), 4);

    verify_table_exists(&result, "HPET");
}

#[test]
fn test_parse_intel_nuc_tables() {
    let reader = intel_nuc7i5dn_phys_mem_reader();
    let result = AcpiParser::init(&reader, reader.rsdp()).unwrap();
    assert_eq!(result.num_tables(), 28);

    verify_table_exists(&result, "HPET");
    verify_table_exists(&result, "DBG2");
}

#[test]
fn test_parse_fuchsia_hypervisor() {
    let reader = fuchsia_hypervisor_phys_mem_reader();
    let result = AcpiParser::init(&reader, reader.rsdp()).unwrap();
    assert_eq!(result.num_tables(), 3);
}

#[test]
fn test_read_missing_table() {
    let reader = qemu_phys_mem_reader();
    let result = AcpiParser::init(&reader, reader.rsdp()).unwrap();

    assert!(get_table_by_signature(&result, AcpiSignature(*b"AAAA")).is_none());
    assert!(result.get_table_at_index(result.num_tables()).is_none());
    assert!(result.get_table_at_index(!0).is_none());
}

#[test]
fn test_acpi_checksum() {
    assert!(acpi_checksum_valid(&[]));

    assert!(acpi_checksum_valid(&[0]));

    assert!(!acpi_checksum_valid(&[52]));

    let mut buffer = [32u8, 0u8];
    assert!(!acpi_checksum_valid(&buffer));
    buffer[1] = acpi_checksum(&buffer);
    assert!(acpi_checksum_valid(&buffer));
}

#[test]
fn test_rsdt_invalid_lengths() {
    let mut bad_rsdt = AcpiRsdt {
        header: AcpiSdtHeader {
            sig: AcpiRsdt::K_SIGNATURE,
            length: 10, // covers checksum, but nothing else.
            revision: 1,
            checksum: 0,
            oemid: [0; 6],
            oem_table_id: [0; 8],
            oem_revision: 0,
            creator_id: 0,
            creator_revision: 0,
        },
    };

    // Calculate checksum first using a temporary borrow
    let temp_bytes =
        unsafe { core::slice::from_raw_parts(&bad_rsdt as *const AcpiRsdt as *const u8, 10) };
    bad_rsdt.header.checksum = acpi_checksum(temp_bytes);

    // Now get the bytes for the region
    let bad_rsdt_bytes = unsafe {
        core::slice::from_raw_parts(
            &bad_rsdt as *const AcpiRsdt as *const u8,
            core::mem::size_of::<AcpiRsdt>(),
        )
    };

    let region = [FakeRegion { phys_addr: 0x1000, data: bad_rsdt_bytes }];

    let reader = FakePhysMemReader::new(0, &region);
    assert!(validate_rsdt(&reader, 0x1000).is_err());
}

#[test]
fn test_dump_tables() {
    let reader = qemu_phys_mem_reader();
    let result = AcpiParser::init(&reader, reader.rsdp());
    assert!(result.is_ok());
    result.unwrap().dump_tables();
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
struct BiosAreaPhysMemReader<'a> {
    bios_area: std::vec::Vec<u8>,
    fallback: FakePhysMemReader<'a>,
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
impl<'a> BiosAreaPhysMemReader<'a> {
    fn new(regions: &[FakeRegion<'a>]) -> Self {
        let mut bios_area = std::vec![0; K_BIOS_READ_ONLY_AREA_LENGTH];
        for region in regions {
            if region.phys_addr >= K_BIOS_READ_ONLY_AREA_START
                && region.phys_addr < K_BIOS_READ_ONLY_AREA_START + K_BIOS_READ_ONLY_AREA_LENGTH
            {
                let offset = region.phys_addr - K_BIOS_READ_ONLY_AREA_START;
                let len = std::cmp::min(region.data.len(), K_BIOS_READ_ONLY_AREA_LENGTH - offset);
                bios_area[offset..offset + len].copy_from_slice(&region.data[..len]);
            }
        }
        Self { bios_area, fallback: FakePhysMemReader::new(0, regions) }
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
impl<'a> PhysMemReader for BiosAreaPhysMemReader<'a> {
    fn phys_to_slice(&self, phys: usize, length: usize) -> Result<&[u8], Status> {
        let end = K_BIOS_READ_ONLY_AREA_START + K_BIOS_READ_ONLY_AREA_LENGTH;
        if phys >= K_BIOS_READ_ONLY_AREA_START && phys < end && phys + length <= end {
            let offset = phys - K_BIOS_READ_ONLY_AREA_START;
            return Ok(&self.bios_area[offset..offset + length]);
        }
        self.fallback.phys_to_slice(phys, length)
    }
}

#[test]
fn test_acpi_signature_construct() {
    let sig = AcpiSignature(*b"ABCD");
    assert_eq!(&sig.0, b"ABCD");
}

#[test]
fn test_acpi_signature_write_to_buffer() {
    let sig = AcpiSignature(*b"ABCD");
    let mut buff = [0u8; 5];
    sig.write_to_buffer(&mut buff);
    assert_eq!(&buff[..4], b"ABCD");
    assert_eq!(buff[4], 0);
}

#[test]
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn test_rsd_ptr_autodetect() {
    let qemu = qemu_phys_mem_reader();
    let reader = BiosAreaPhysMemReader::new(qemu.regions());
    let result = AcpiParser::init(&reader, 0).unwrap();
    assert_eq!(result.num_tables(), 4);
}

#[test]
fn test_get_table_by_type_nothing_found() {
    let parser = FakeAcpiParser::new(&[]);
    assert!(get_table_by_type::<AcpiHpetTable>(&parser).is_none());
}

#[test]
fn test_get_table_by_type_valid_entry_found() {
    let mut table = AcpiHpetTable {
        header: AcpiSdtHeader {
            sig: AcpiHpetTable::SIGNATURE,
            length: core::mem::size_of::<AcpiHpetTable>() as u32,
            revision: 1,
            checksum: 0,
            oemid: [0; 6],
            oem_table_id: [0; 8],
            oem_revision: 0,
            creator_id: 0,
            creator_revision: 0,
        },
        id: 0,
        address: crate::structures::AcpiGenericAddress {
            address_space_id: 0,
            register_bit_width: 0,
            register_bit_offset: 0,
            access_size: 0,
            address: 0,
        },
        sequence: 0,
        minimum_tick: 0,
        flags: 42,
    };

    let temp_bytes = unsafe {
        core::slice::from_raw_parts(
            &table as *const AcpiHpetTable as *const u8,
            core::mem::size_of::<AcpiHpetTable>(),
        )
    };
    table.header.checksum = acpi_checksum(temp_bytes);

    let table_bytes = unsafe {
        core::slice::from_raw_parts(
            &table as *const AcpiHpetTable as *const u8,
            core::mem::size_of::<AcpiHpetTable>(),
        )
    };

    let parser = FakeAcpiParser::new(&[table_bytes]);
    let result = get_table_by_type::<AcpiHpetTable>(&parser).unwrap();
    assert_eq!(result.flags, 42);
}

#[test]
fn test_get_table_by_type_short_entry() {
    let mut table = AcpiHpetTable {
        header: AcpiSdtHeader {
            sig: AcpiHpetTable::SIGNATURE,
            // Length is too short to hold a |AcpiHpetTable|.
            length: (core::mem::size_of::<AcpiHpetTable>() - 1) as u32,
            revision: 1,
            checksum: 0,
            oemid: [0; 6],
            oem_table_id: [0; 8],
            oem_revision: 0,
            creator_id: 0,
            creator_revision: 0,
        },
        id: 0,
        address: crate::structures::AcpiGenericAddress {
            address_space_id: 0,
            register_bit_width: 0,
            register_bit_offset: 0,
            access_size: 0,
            address: 0,
        },
        sequence: 0,
        minimum_tick: 0,
        flags: 42,
    };

    let temp_bytes = unsafe {
        core::slice::from_raw_parts(
            &table as *const AcpiHpetTable as *const u8,
            core::mem::size_of::<AcpiHpetTable>() - 1,
        )
    };
    table.header.checksum = acpi_checksum(temp_bytes);

    let table_bytes = unsafe {
        core::slice::from_raw_parts(
            &table as *const AcpiHpetTable as *const u8,
            core::mem::size_of::<AcpiHpetTable>() - 1,
        )
    };

    let parser = FakeAcpiParser::new(&[table_bytes]);
    assert!(get_table_by_type::<AcpiHpetTable>(&parser).is_none());
}

#[test]
fn test_xsdt_get_entry() {
    let reader = intel_nuc7i5dn_phys_mem_reader();
    let xsdt_slice = reader.phys_to_slice(0x7fa290c0, 260).unwrap();
    let xsdt = unsafe { &*(xsdt_slice.as_ptr() as *const crate::structures::AcpiXsdt) };
    assert_eq!(xsdt.header.size(), 260);
    assert_eq!(unsafe { xsdt.get_entry(0) }, 0x7fa53130);
    assert_eq!(unsafe { xsdt.get_entry(1) }, 0x7fa53248);
    assert_eq!(unsafe { xsdt.get_entry(27) }, 0x7fa63d18);
}
