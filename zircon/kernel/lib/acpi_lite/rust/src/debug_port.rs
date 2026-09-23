// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::binary_reader::{BinaryReader, Unaligned};
use crate::structures::{
    ACPI_ADDR_SPACE_IO, ACPI_ADDR_SPACE_MEMORY, ACPI_DBG2_SUBTYPE_16550_COMPATIBLE,
    ACPI_DBG2_TYPE_SERIAL_PORT, AcpiDbg2Device, AcpiDbg2Table, AcpiGenericAddress,
};
use crate::{AcpiParserInterface, get_table_by_type};
use kprint::kprintln;
use zx_status::Status;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AcpiDebugPortType {
    Mmio,
    Pio,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
// Describes a dedicated system debug port suitable for low-level
// debugging and diagnostics.
//
// Currently, we only support a 16550-compatible UART using MMIO or PIO.
pub struct AcpiDebugPortDescriptor {
    pub r#type: AcpiDebugPortType,

    // Physical address of the 16550 MMIO registers for Type::Mmio.
    // IO port base for Type::Pio.
    pub address: usize,
    pub length: u32,
}

// Parse an AcpiDbg2Table ACPI structure.
pub fn parse_acpi_dbg2_table(
    debug_table: &AcpiDbg2Table,
) -> Result<AcpiDebugPortDescriptor, Status> {
    let num_entries = debug_table.num_entries;
    if num_entries < 1 {
        kprintln!("acpi_lite: DBG2 table contains no debug ports.");
        return Err(Status::NOT_FOUND);
    }

    let offset = debug_table.offset as usize;
    let mut reader = BinaryReader::from_variable_sized(debug_table);
    if !reader.skip_bytes(offset) {
        return Err(Status::INTERNAL);
    }

    let device = reader.read::<AcpiDbg2Device>().ok_or(Status::INTERNAL)?;

    let port_type = device.port_type;
    let port_subtype = device.port_subtype;
    if port_type != ACPI_DBG2_TYPE_SERIAL_PORT || port_subtype != ACPI_DBG2_SUBTYPE_16550_COMPATIBLE
    {
        kprintln!(
            "acpi_lite: DBG2 debug port unsupported. (type={:x}, subtype={:x})",
            port_type,
            port_subtype,
        );
        return Err(Status::NOT_SUPPORTED);
    }

    if device.register_count < 1 {
        kprintln!("acpi_lite: DBG2 debug port doesn't have any registers defined.");
        return Err(Status::NOT_SUPPORTED);
    }

    let base_address_offset = device.base_address_offset as usize;
    let mut reader = BinaryReader::from_variable_sized(device);
    if !reader.skip_bytes(base_address_offset) {
        return Err(Status::INTERNAL);
    }
    let address = reader.read_fixed_length::<AcpiGenericAddress>().ok_or(Status::INTERNAL)?;

    let address_size_offset = device.address_size_offset as usize;
    let mut reader = BinaryReader::from_variable_sized(device);
    if !reader.skip_bytes(address_size_offset) {
        return Err(Status::INTERNAL);
    }
    let length = reader.read_fixed_length::<Unaligned<u32>>().ok_or(Status::INTERNAL)?;

    let addr = address.address as usize;
    let mut result = AcpiDebugPortDescriptor {
        r#type: AcpiDebugPortType::Mmio,
        address: addr,
        length: length.0,
    };

    match address.address_space_id {
        ACPI_ADDR_SPACE_MEMORY => {
            result.r#type = AcpiDebugPortType::Mmio;
        }
        ACPI_ADDR_SPACE_IO => {
            result.r#type = AcpiDebugPortType::Pio;
        }
        _ => {
            kprintln!(
                "acpi_lite: Address space unsupported (space_id={:x})",
                address.address_space_id,
            );
            return Err(Status::NOT_SUPPORTED);
        }
    }

    Ok(result)
}

// Lookup low-level debug port information.
pub fn get_debug_port(parser: &dyn AcpiParserInterface) -> Result<AcpiDebugPortDescriptor, Status> {
    let debug_table = get_table_by_type::<AcpiDbg2Table>(parser).ok_or_else(|| {
        kprintln!("acpi_lite: could not find debug port (v2) ACPI entry");
        Status::NOT_FOUND
    })?;

    parse_acpi_dbg2_table(debug_table)
}
